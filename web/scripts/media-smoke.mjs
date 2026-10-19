// Isolated, real HTTP/WebSocket/S3 check. All runtime files stay under tmp/.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";
import { parseFrame } from "../src/api/protocol.ts";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
await mkdir(join(root, "tmp"), { recursive: true });
const work = await mkdtemp(join(root, "tmp/media-smoke-"));
const origin = "http://127.0.0.1:5173";
const children = [];
const sockets = [];
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
async function until(predicate, label) {
  const deadline = Date.now() + 30000;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`Timed out: ${label}`);
    await pause(25);
  }
}
async function port() {
  const server = createServer();
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const result = server.address().port;
  await new Promise((done) => server.close(done));
  return result;
}
function launch(binary, args, env) {
  const child = spawn(binary, args, {
    cwd: work,
    env,
    stdio: ["pipe", "pipe", "pipe"],
  });
  children.push(child);
  child.output = "";
  child.stdout.on("data", (data) => {
    child.output += data.toString();
  });
  child.stderr.on("data", (data) => {
    child.output += data.toString();
  });
  child.on("error", (error) => {
    child.failure = error;
  });
  return child;
}
const env = { ...process.env };
for (const name of Object.keys(env))
  if (name.startsWith("CHAT_") || name.startsWith("RUSTFS_")) delete env[name];
Object.assign(env, {
  RUST_LOG: "chat=info",
  CHAT_S3_ACCESS_KEY: "media-smoke-local",
  CHAT_S3_SECRET_KEY: "media-smoke-local-secret-only",
  CHAT_S3_BUCKET: "media-smoke",
  CHAT_S3_PATH_STYLE: "true",
  CHAT_S3_ENDPOINT: `http://127.0.0.1:${await port()}`,
  CHAT_BIND: "127.0.0.1:0",
  CHAT_BASE_URL: "/commonroom/",
  CHAT_DATA: join(work, "chat"),
  RUSTFS_ACCESS_KEY: "media-smoke-local",
  RUSTFS_SECRET_KEY: "media-smoke-local-secret-only",
  RUSTFS_LOG_DIRECTORY: join(work, "logs"),
  RUSTFS_OBS_LOG_DIRECTORY: join(work, "logs"),
  TMPDIR: work,
  TMP: work,
  TEMP: work,
  RUSTFS_CONSOLE_ENABLE: "false",
});
let base;
async function request(path, cookie, body, extra = {}) {
  return fetch(`${base}${path}`, {
    method: body === undefined ? "GET" : "POST",
    redirect: "manual",
    headers: {
      Origin: origin,
      ...(cookie ? { Cookie: cookie } : {}),
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
      ...extra,
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
}
async function login(username) {
  const response = await request("/api/login", null, {
    username,
    password: "smoke-test-only",
  });
  assert.equal(response.status, 200);
  return response.headers.get("set-cookie").split(";")[0];
}
async function socket(cookie) {
  const ws = new WebSocket(`${base.replace("http:", "ws:")}/ws`, {
    headers: { Cookie: cookie, Origin: origin },
  });
  sockets.push(ws);
  const frames = [];
  ws.on("message", (raw) => {
    const frame = parseFrame(raw.toString());
    assert.ok(frame);
    frames.push(frame);
  });
  await once(ws, "open");
  await until(
    () => frames.some((frame) => frame.kind === "snapshot"),
    "socket baseline",
  );
  let serial = 0;
  return async (text, room = "files") => {
    const id = ++serial;
    ws.send(JSON.stringify({ id, room, text, markdown: true }));
    await until(
      () =>
        frames.some(
          (frame) =>
            frame.id === id && ["notice", "error"].includes(frame.kind),
        ),
      "message acknowledgement",
    );
    const result = frames.find(
      (frame) => frame.id === id && ["notice", "error"].includes(frame.kind),
    );
    assert.equal(result.kind, "notice", result.text);
  };
}
try {
  await mkdir(join(work, "objects"));
  const storageArgs = [
    join(root, "tmp/rustfs"),
    "server",
    "--address",
    new URL(env.CHAT_S3_ENDPOINT).host,
    join(work, "objects"),
  ];
  const launchStorage = () =>
    process.platform === "darwin"
      ? launch(
          "sandbox-exec",
          [
            "-p",
            `(version 1)(allow default)(deny file-write*)(allow file-write* (subpath ${JSON.stringify(join(root, "tmp"))}))(allow file-write* (literal "/dev/null") (literal "/dev/tty"))`,
            ...storageArgs,
          ],
          env,
        )
      : process.platform === "linux"
        ? launch(
            "bwrap",
            [
              "--die-with-parent",
              "--ro-bind",
              "/",
              "/",
              "--bind",
              join(root, "tmp"),
              join(root, "tmp"),
              "--dev",
              "/dev",
              "--proc",
              "/proc",
              ...storageArgs,
            ],
            env,
          )
        : (() => {
            throw new Error(
              "RustFS smoke confinement requires macOS or Linux.",
            );
          })();
  const rustfs = launchStorage();
  let ready = false;
  for (let attempt = 0; attempt < 100 && !ready; attempt++) {
    if (rustfs.failure || rustfs.exitCode !== null)
      throw new Error(
        "RustFS startup failed. See the local binary/configuration.",
      );
    ready = await fetch(env.CHAT_S3_ENDPOINT).then(
      () => true,
      () => false,
    );
    if (!ready) await pause(100);
  }
  assert.ok(ready, "RustFS startup timeout");
  // Exercise the exact setup used by debug.sh, including an existing bucket.
  for (let attempt = 0; attempt < 2; attempt++) {
    const setup = launch(
      join(root, "target/debug/chat"),
      ["--setup-local-s3"],
      env,
    );
    await once(setup, "exit");
    assert.equal(setup.exitCode, 0, setup.output);
  }
  const chat = launch(join(root, "target/debug/chat"), [], env);
  await until(
    () =>
      chat.failure ||
      chat.exitCode !== null ||
      chat.output.includes("Chat server ready"),
    "chat startup",
  );
  assert.equal(chat.exitCode, null, chat.output);
  base = `http://127.0.0.1:${chat.output.match(/address=127\.0\.0\.1:(\d+)/)[1]}/commonroom`;
  chat.stdin.write(
    "/new user alice smoke-test-only admin\n/new user bob smoke-test-only user\n/new user eve smoke-test-only user\n/new room files\n/add alice files\n/add bob files\n",
  );
  await until(
    () => chat.output.includes("bob added to #files"),
    "test accounts",
  );
  const alice = await login("alice"),
    bob = await login("bob"),
    eve = await login("eve");
  const send = await socket(alice);
  const snapshot = await (await request("/api/me", alice)).json();
  const conversation = snapshot.rooms.find((room) => room.name === "files").id;
  assert.deepEqual(await (await request("/api/media/config", alice)).json(), {
    enabled: true,
    max_size: 25 * 1024 * 1024,
  });
  assert.equal(
    (
      await request("/api/media/uploads", eve, {
        conversation_id: conversation,
        filename: "file",
        size: 4,
      })
    ).status,
    400,
  );
  const allocation = await request("/api/media/uploads", alice, {
    conversation_id: conversation,
    filename: "note.txt",
    size: 4,
  });
  assert.equal(allocation.status, 200);
  const ticket = await allocation.json();
  const preflight = await fetch(ticket.upload.url, {
    method: "OPTIONS",
    headers: {
      Origin: origin,
      "Access-Control-Request-Method": "PUT",
      "Access-Control-Request-Headers": "content-type",
    },
  });
  assert.ok(preflight.ok);
  assert.equal(preflight.headers.get("access-control-allow-origin"), origin);
  assert.equal(
    (
      await fetch(ticket.upload.url, {
        method: "PUT",
        headers: ticket.upload.headers,
        body: "test",
      })
    ).status,
    200,
  );
  const completePath = `/api/media/uploads/${ticket.media_id}/complete`;
  assert.equal((await request(completePath, bob, {})).status, 400);
  const completed = await request(completePath, alice, {});
  assert.equal(completed.status, 200);
  const media = await completed.json();
  assert.deepEqual(
    await (await request(completePath, alice, {})).json(),
    media,
  );
  const path = media.path.replace("/commonroom", "");
  assert.equal((await request(path, alice)).status, 404);
  await send(`[note](${new URL(media.path, base).href})`);
  const response = await request(path, bob);
  assert.equal(response.status, 307);
  assert.equal(response.headers.get("cache-control"), "no-store");
  const downloaded = await fetch(response.headers.get("location"));
  assert.equal(await downloaded.text(), "test");
  assert.match(downloaded.headers.get("content-disposition"), /^attachment;/);
  assert.equal((await request(path, eve)).status, 404);
  assert.equal((await request(path, null)).status, 401);
  const history = await (
    await request("/api/history?view=files", alice)
  ).json();
  await send(`/retract ${history.messages[0].id}`);
  assert.equal((await request(path, bob)).status, 404);
  assert.equal(
    (
      await request(
        "/api/media/uploads",
        alice,
        { conversation_id: conversation, filename: "bad", size: 4 },
        { Origin: "https://evil.test" },
      )
    ).status,
    403,
  );
  const stopped = once(rustfs, "exit");
  rustfs.kill("SIGTERM");
  await stopped;
  assert.equal(
    (await (await request("/api/media/config", alice)).json()).enabled,
    true,
    "Configured uploads stay visible during an outage",
  );
  for (let attempt = 0; attempt < 9; attempt++) {
    const rejected = await request("/api/media/uploads", alice, {
      conversation_id: conversation,
      filename: "offline.txt",
      size: 4,
    });
    assert.equal(rejected.status, 503);
    assert.match(
      (await rejected.json()).error,
      /Upload rejected.*storage is unavailable/,
    );
  }
  assert.equal(
    (
      await request("/api/media/uploads", eve, {
        conversation_id: conversation,
        filename: "offline.txt",
        size: 4,
      })
    ).status,
    400,
    "Authorization precedes the storage probe",
  );
  launchStorage();
  let recovered = false;
  for (let attempt = 0; attempt < 100 && !recovered; attempt++) {
    recovered = await fetch(env.CHAT_S3_ENDPOINT).then(
      () => true,
      () => false,
    );
    if (!recovered) await pause(100);
  }
  assert.ok(recovered, "RustFS recovers");
  const recoveredTicket = await request("/api/media/uploads", alice, {
    conversation_id: conversation,
    filename: "recovered.txt",
    size: 4,
  });
  assert.equal(
    recoveredTicket.status,
    200,
    "Rejected uploads did not consume pending slots",
  );
  console.log(
    "Media smoke passed: RustFS, base paths, auth, upload, idempotent completion, message binding, private downloads, retraction, origin checks, storage outage rejection, and recovery.",
  );
} finally {
  for (const ws of sockets) ws.terminate();
  for (const child of children.reverse()) {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill("SIGTERM");
      await once(child, "exit");
    }
  }
  await rm(work, { recursive: true, force: true });
}
