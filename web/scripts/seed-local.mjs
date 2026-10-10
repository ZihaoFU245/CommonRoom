// One-shot local seeding script for a manual trial run.
//
//   DEEPSEEK_API_KEY=sk-... node scripts/seed-local.mjs <data-directory>
//
// Starts the debug binary on a private port, provisions the demo accounts over
// the engine's stdin console, then signs in as the admin and drives the real
// HTTP/WebSocket path so the owner-scoped agent commands run exactly as a
// browser would. It stops the temporary server when the data folder is ready;
// the caller then starts the server normally.
import { spawn } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import WebSocket from "ws";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binary = join(
  root,
  "target/debug",
  process.platform === "win32" ? "chat.exe" : "chat",
);
const data = process.argv[2];
if (!data) {
  console.error("usage: node scripts/seed-local.mjs <data-directory>");
  process.exit(1);
}

const KEY = process.env.DEEPSEEK_API_KEY || "";
const ORIGIN = "http://localhost:5173";
const ACCOUNTS = [
  ["alice", "alice-long-password", "admin"],
  ["bob", "bob-long-password", "user"],
];

await mkdir(data, { recursive: true });
const configPath = join(data, "config.json");
const config = JSON.parse(await readFile(configPath, "utf8").catch(() => "{}"));
config.bind = "127.0.0.1:3901";
config.origins = [ORIGIN, "http://127.0.0.1:5173", "http://127.0.0.1:3000"];
config.production = false;
await writeFile(configPath, JSON.stringify(config, null, 2));

let logs = "";
let child;
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
async function until(predicate, label) {
  const start = Date.now();
  while (!predicate()) {
    if (Date.now() - start > 20000)
      throw new Error(`Timed out: ${label}\n${logs}`);
    await pause(30);
  }
}
try {
  child = spawn(binary, [], {
    cwd: root,
    env: { ...process.env, CHAT_DATA: data, RUST_LOG: "chat=info" },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stdout.on("data", (chunk) => {
    logs += chunk.toString();
  });
  child.stderr.on("data", (chunk) => {
    logs += chunk.toString();
  });
  await until(() => /Chat server ready/.test(logs), "server startup");
  const base = `http://127.0.0.1:3901`;

  // The engine console may create people; it deliberately refuses to create
  // agents, because an agent needs an owning account.
  for (const [name, password, role] of ACCOUNTS) {
    const before = logs.length;
    child.stdin.write(`/user ${name} ${password} ${role}\n`);
    await until(() => logs.length > before, `provision ${name}`);
    await pause(150);
  }
  child.stdin.write("/new demo\n");
  await pause(400);
  child.stdin.write("/add alice demo\n");
  await pause(400);

  const cookieFor = async (name, password) => {
    const response = await fetch(`${base}/api/login`, {
      method: "POST",
      headers: { Origin: ORIGIN, "Content-Type": "application/json" },
      body: JSON.stringify({ username: name, password }),
    });
    if (!response.ok) throw new Error(`login ${name}: ${response.status}`);
    return response.headers.get("set-cookie").split(";")[0];
  };
  const socket = new WebSocket(`${base.replace("http:", "ws:")}/ws`, {
    headers: {
      Origin: ORIGIN,
      Cookie: await cookieFor(...ACCOUNTS[0].slice(0, 2)),
    },
  });
  let snapshot = null;
  const frames = new Map();
  socket.on("message", (raw) => {
    const frame = JSON.parse(raw.toString());
    if (frame.kind === "snapshot") snapshot = frame;
    if (frame.id !== undefined) frames.set(frame.id, frame);
  });
  await until(() => snapshot, "initial snapshot");
  let serial = 0;
  const send = async (text, room = null, kind = "notice") => {
    const id = ++serial;
    socket.send(JSON.stringify({ id, room, text }));
    await until(() => frames.has(id), `acknowledgement for ${text}`);
    const reply = frames.get(id);
    if (reply.kind !== kind)
      throw new Error(`${text} -> ${reply.kind}: ${reply.text}`);
    return reply.text;
  };

  for (const [name, password] of ACCOUNTS) {
    child.stdin.write(`/add ${name} demo\n`);
    await pause(400);
    void password;
  }
  if (KEY) {
    await send(`/agent helper ${KEY}`, null);
    await send("/agent-reply mention helper", null);
    await send("/add helper demo", null);
  }
  if (KEY && logs.includes(KEY)) throw new Error("the key reached the log");
  const agents = Object.entries(snapshot.roles || {})
    .filter(([, role]) => role === "agent")
    .map(([name]) => name);
  console.log(
    `ready: ${data}\n  alice / alice-long-password  (admin)\n  bob   / bob-long-password    (user)\n  room  #demo\n  agents: ${agents.join(", ") || "none — /agent is web-only, create one in the browser"}${KEY ? " (key configured)" : ""}`,
  );
  socket.close();
} finally {
  if (child && child.exitCode === null) {
    await new Promise((done) => {
      child.once("close", done);
      child.kill("SIGKILL");
      setTimeout(done, 4000);
    });
  }
}
