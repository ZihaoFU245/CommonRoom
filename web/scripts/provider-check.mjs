import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binary = join(
  root,
  "target/debug",
  process.platform === "win32" ? "chat.exe" : "chat",
);
const ORIGIN = "http://localhost:5173";
const temporary = await mkdtemp(join(tmpdir(), "commonroom-provider-"));
let child;
let logs = "";
const pause = (ms) => new Promise((d) => setTimeout(d, ms));
async function until(predicate, label, ms = 30000) {
  const start = Date.now();
  while (!predicate()) {
    if (Date.now() - start > ms) throw new Error(`timeout: ${label}\n${logs}`);
    await pause(40);
  }
}
try {
  const data = join(temporary, "data");
  await mkdir(data);
  await writeFile(
    join(data, "config.json"),
    JSON.stringify({
      bind: "127.0.0.1:3911",
      origins: [ORIGIN],
      production: false,
    }),
  );
  child = spawn(binary, [], {
    cwd: root,
    env: { ...process.env, CHAT_DATA: data, RUST_LOG: "chat=info" },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stdout.on("data", (c) => {
    logs += c.toString();
  });
  child.stderr.on("data", (c) => {
    logs += c.toString();
  });
  await until(() => /Chat server ready/.test(logs), "startup");
  child.stdin.write("/user alice alice-long-password admin\n");
  await until(() => logs.includes("Account alice created."), "alice");
  const base = "http://127.0.0.1:3911";
  const login = await fetch(`${base}/api/login`, {
    method: "POST",
    headers: { Origin: ORIGIN, "Content-Type": "application/json" },
    body: JSON.stringify({
      username: "alice",
      password: "alice-long-password",
    }),
  });
  const cookie = login.headers.get("set-cookie").split(";")[0];
  const ws = new WebSocket(`${base.replace("http:", "ws:")}/ws`, {
    headers: { Origin: ORIGIN, Cookie: cookie },
  });
  let snapshot = null;
  const frames = new Map();
  ws.on("message", (raw) => {
    const f = JSON.parse(raw.toString());
    if (f.kind === "snapshot") snapshot = f;
    if (f.id !== undefined) frames.set(f.id, f);
  });
  await until(() => snapshot, "snapshot");
  let serial = 0;
  const send = async (text, room = null, kind = "notice") => {
    const id = ++serial;
    ws.send(JSON.stringify({ id, room, text }));
    await until(() => frames.has(id), `ack ${text}`);
    const r = frames.get(id);
    assert.equal(r.kind, kind, `${text} -> ${r.text}`);
    return r.text;
  };
  await send("/agent gateway sk-mock-key");
  await send(`/agent-base-url http://127.0.0.1:9/v1 gateway`, null, "error");
  await send(`/agent-base-url https://127.0.0.1:9/v1 gateway`);
  await send(`/agent-provider openrouter gateway`);
  await send(`/agent-model anthropic/claude-3.5-mini gateway`);
  await send(`/agent-reply mention gateway`);
  await send("/new lobby");
  await send("/add gateway lobby");
  const config = await send("/agent-config gateway");
  assert.match(config, /openrouter · anthropic\/claude-3\.5-mini/);
  assert.match(
    config,
    /base URL: https:\/\/127\.0\.0\.1:9\/v1/,
    "the base URL wins and is reported",
  );
  assert.match(config, /reply: mention/);
  assert.match(config, /provider key: set/);
  assert.match(config, /search key: missing/);
  assert.ok(!config.includes("sk-mock-key"), "no credential in the summary");
  // A request bound for the unreachable base URL fails as unreachable, which
  // proves the stored base URL is what the request used.
  await send("@gateway 你好", "lobby");
  await until(
    () =>
      snapshot.rooms
        .find((r) => r.name === "lobby")
        .messages.some((m) => m.from === "gateway"),
    "an answer arrives",
    90000,
  );
  const answer = snapshot.rooms
    .find((r) => r.name === "lobby")
    .messages.filter((m) => m.from === "gateway")
    .at(-1);
  assert.match(
    answer.text,
    /^⚠ The model provider could not be reached\./,
    answer.text,
  );
  assert.ok(!logs.includes("sk-mock-key"), "the key never reaches the log");
  // Returning to the default provider drops the overrides.
  await send("/agent-base-url - gateway");
  await send("/agent-model - gateway");
  await send("/agent-provider - gateway");
  const reset = await send("/agent-config gateway");
  assert.match(reset, /deepseek · deepseek-flash/, reset);
  ws.close();
  console.log(
    "PASS: provider, base URL, and model are stored, validated, reported, and used by the request; no credential is exposed",
  );
} finally {
  if (child && child.exitCode === null) {
    await new Promise((done) => {
      child.once("close", done);
      child.kill("SIGKILL");
      setTimeout(done, 4000);
    });
  }
  await rm(temporary, { recursive: true, force: true });
}
