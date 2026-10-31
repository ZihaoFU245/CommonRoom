#!/usr/bin/env python3
"""Exercise the real binary's offline transfer and PostgreSQL HTTP lifecycle.

Requires CHAT_TEST_POSTGRES_URL pointing to an EMPTY disposable PostgreSQL target.
Build target/debug/chat first. This script creates chat tables in that target;
remove the disposable container/database afterward. All SQLite/local files are
temporary. It never opens the repository's data/ directory.
"""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
ORIGIN = "http://localhost:5173"


def request(port, path, cookie=None, payload=None):
    headers = {"Origin": ORIGIN}
    if cookie:
        headers["Cookie"] = cookie
    data = None
    if payload is not None:
        data = json.dumps(payload).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=data, headers=headers)
    with urllib.request.urlopen(req, timeout=3) as response:
        return json.load(response), response.headers.get("Set-Cookie")


def start(binary, env, log):
    process = subprocess.Popen([str(binary)], cwd=ROOT, env=env, stdin=subprocess.PIPE,
                               stdout=log, stderr=subprocess.STDOUT, text=True)
    deadline = time.monotonic() + 15
    port = int(env["CHAT_BIND"].rsplit(":", 1)[1])
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Server exited before readiness; inspect the smoke log")
        try:
            body, _ = request(port, "/api/health")
            if body.get("ok"):
                return process
        except (OSError, urllib.error.URLError):
            time.sleep(0.05)
    stop(process)
    raise RuntimeError("Server readiness timed out")


def stop(process):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise RuntimeError("Graceful shutdown timed out")
    if process.returncode != 0:
        raise RuntimeError(f"Server exited with {process.returncode}")


def check_second_owner(binary, env, data):
    original = (data / "config.json").read_bytes()
    competing = dict(env, CHAT_BIND="127.0.0.1:0")
    result = subprocess.run([str(binary)], cwd=ROOT, env=competing,
                            stdin=subprocess.DEVNULL, capture_output=True, timeout=10)
    assert result.returncode != 0, "Second server acquired the same data folder"
    assert (data / "config.json").read_bytes() == original, "Rejected server rewrote configuration"


def check_runtime_limits(process, port, log_path):
    # Exercise configured transport deadlines through the real binary, on either backend.
    with socket.create_connection(("127.0.0.1", port), timeout=3) as slow:
        slow.sendall(b"POST /api/login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{")
        response = bytearray()
        while chunk := slow.recv(4096):
            response.extend(chunk)
        assert b"408 Request Timeout" in response
        assert b"cache-control: no-store" in response
    offset = log_path.stat().st_size
    process.stdin.write("/system metrics --txt\n")
    process.stdin.flush()
    expected = ["io_jobs_limit: 4", "password_jobs_limit: 1", "max_http_connections: 8",
                "admission.http_per_account_limit: 2", "admission.browser_sockets_limit: 8",
                "admission.bot_sockets_limit: 4", "admission.sockets_per_account_limit: 2",
                "admission.http.body_timeouts: 1", "ordering.metadata.completed:"]
    deadline = time.monotonic() + 5
    while True:
        report = log_path.read_bytes()[offset:].decode()
        if all(item in report for item in expected):
            break
        if time.monotonic() >= deadline:
            raise RuntimeError("Configured capacities/new runtime metrics missing from console report: " + report)
        time.sleep(0.05)


def main():
    destination = os.environ.get("CHAT_TEST_POSTGRES_URL")
    if not destination:
        raise SystemExit("Set CHAT_TEST_POSTGRES_URL to an empty disposable PostgreSQL database")
    binary = Path(os.environ.get("CHAT_BIN", ROOT / "target/debug/chat")).resolve()
    if not binary.is_file():
        raise SystemExit("Build the server first: cargo build --locked")
    with tempfile.TemporaryDirectory(prefix="chat-postgres-smoke-") as folder:
        folder = Path(folder)
        data = folder / "data"
        data.mkdir()
        # Non-default budgets must survive startup, backend transfer and restarts.
        (data / "config.json").write_text(json.dumps({"server": {
            "max_http_connections": 8, "http_per_account": 2, "io_jobs": 4,
            "password_jobs": 1, "browser_sockets": 8, "bot_sockets": 4,
            "sockets_per_account": 2, "header_timeout_seconds": 2,
            "body_timeout_seconds": 1, "write_timeout_seconds": 2,
            "request_timeout_seconds": 120, "ordering_timeout_seconds": 2,
        }}))
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        env = dict(os.environ, CHAT_DATA=str(data), CHAT_BIND=f"127.0.0.1:{port}", CHAT_PRODUCTION="0")
        # A URL alone must never override the default/explicit SQLite backend.
        env["CHAT_DATABASE_URL"] = destination
        for key in tuple(env):
            if key.startswith("CHAT_S3_"):
                del env[key]
        path = data / "chat.sqlite"
        log_path = folder / "server.log"
        process = None
        try:
            with log_path.open("w+") as log:
                process = start(binary, env, log)
                process.stdin.write("/user new alice smoke-password admin\n/room new smoke\n/room add alice #smoke\n")
                process.stdin.flush()
                deadline = time.monotonic() + 10
                while True:
                    try:
                        _, cookie = request(port, "/api/login", payload={"username": "alice", "password": "smoke-password"})
                        break
                    except urllib.error.HTTPError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(0.1)
                cookie = cookie.split(";", 1)[0]
                deadline = time.monotonic() + 5
                while True:
                    snapshot, _ = request(port, "/api/me", cookie)
                    if any(room["name"] == "smoke" for room in snapshot["rooms"]):
                        break
                    if time.monotonic() >= deadline:
                        raise RuntimeError("Console provisioning did not finish")
                    time.sleep(0.05)
                check_runtime_limits(process, port, log_path)
                check_second_owner(binary, env, data)
                stop(process)
                process = None
                original = hashlib.sha256(path.read_bytes()).digest()
                env["CHAT_DATABASE_URL"] = destination
                migration = subprocess.run([str(binary), "--migrate-sqlite", str(path)],
                                           cwd=ROOT, env=env, text=True, capture_output=True, timeout=30)
                if migration.returncode:
                    raise RuntimeError("Migration command failed: " + migration.stderr)
                assert "Migration committed" in migration.stdout
                assert hashlib.sha256(path.read_bytes()).digest() == original
                config_path=data / "config.json"
                config=json.loads(config_path.read_text())
                assert config["database"] == {"backend":"sqlite"}
                config["database"]={"backend":"postgres"}
                config_path.write_text(json.dumps(config))
                original_config=config_path.read_bytes()
                assert destination not in original_config.decode()
                for missing_url in [None, "", "  "]:
                    invalid_env=dict(env)
                    if missing_url is None:
                        invalid_env.pop("CHAT_DATABASE_URL",None)
                    else:
                        invalid_env["CHAT_DATABASE_URL"]=missing_url
                    rejected=subprocess.run([str(binary)],cwd=ROOT,env=invalid_env,
                                            stdin=subprocess.DEVNULL,capture_output=True,timeout=10)
                    assert rejected.returncode != 0, "PostgreSQL silently fell back to SQLite"
                    assert b"requires CHAT_DATABASE_URL" in rejected.stderr
                    assert config_path.read_bytes() == original_config
                    assert hashlib.sha256(path.read_bytes()).digest() == original
                for _ in range(2):
                    process = start(binary, env, log)
                    snapshot, _ = request(port, "/api/me", cookie)
                    assert snapshot["username"] == "alice"
                    assert any(room["name"] == "smoke" for room in snapshot["rooms"])
                    _, new_cookie = request(port, "/api/login", payload={"username": "alice", "password": "smoke-password"})
                    assert new_cookie
                    check_runtime_limits(process, port, log_path)
                    check_second_owner(binary, env, data)
                    stop(process)
                    process = None
                assert hashlib.sha256(path.read_bytes()).digest() == original
                with sqlite3.connect(f"file:{path}?mode=ro", uri=True) as db:
                    assert db.execute("PRAGMA user_version").fetchone()[0] == 20
                print("Passed: real-binary SQLite provisioning, verified migration, preserved session/password, PostgreSQL HTTP startup/restart/shutdown, second-owner config protection, unchanged SQLite source, configured admission/deadlines/runtime metrics")
        except BaseException:
            if process is not None:
                stop(process)
            print(log_path.read_text())
            raise


if __name__ == "__main__":
    main()
