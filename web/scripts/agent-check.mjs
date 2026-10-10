// Manual agent verification against a real server process.
//
// The release smoke harness cannot terminate its child process on Windows, so
// this script drives the same HTTP/WebSocket contract directly: it starts the
// debug binary in a throwaway data folder, exercises agent creation, invites,
// mentions, auto mode, renames, and secret handling, then stops the server.
//
//   node scripts/agent-check.mjs
//
// Set CHAT_LIVE_AGENT_KEY (and CHAT_LIVE_SEARCH_KEY) to exercise the real
// provider and search round trips.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";
import { parseFrame } from "../src/api/protocol.ts";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const binary = join(
  root,
  "target/debug",
  process.platform === "win32" ? "chat.exe" : "chat",
);
const origin = "http://localhost:5173";
const liveKey = process.env.CHAT_LIVE_AGENT_KEY || "";
const liveSearchKey = process.env.CHAT_LIVE_SEARCH_KEY || "";
const temporary = await mkdtemp(join(tmpdir(), "commonroom-agent-"));
const sockets = [];
let child;
let base;
let logs = "";
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(predicate, label) {
  const start = Date.now();
  while (!predicate()) {
    if (Date.now() - start > 20000)
      throw new Error(`Timed out: ${label}\n${logs}`);
    await pause(40);
  }
}
async function start(data) {
  logs = "";
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
  const match = logs.match(/address=127\.0\.0\.1:(\d+)/);
  assert.ok(match, logs);
  base = `http://127.0.0.1:${match[1]}`;
}
async function stop() {
  for (const socket of sockets.splice(0)) socket.terminate();
  if (!child || child.exitCode !== null) return;
  const current = child;
  await new Promise((resolve) => {
    current.once("close", resolve);
    current.kill("SIGKILL");
    setTimeout(resolve, 4000);
  });
  child = null;
}
async function request(path, cookie, body) {
  return fetch(`${base}/api/${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      Origin: origin,
      ...(cookie ? { Cookie: cookie } : {}),
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
}
async function login(username, password) {
  const response = await request("login", null, { username, password });
  assert.equal(response.status, 200, await response.clone().text());
  return response.headers.get("set-cookie").split(";")[0];
}
async function connect(cookie) {
  const client = { ws: null, frames: [], serial: 0, snapshot: null };
  client.ws = new WebSocket(`${base.replace("http:", "ws:")}/ws`, {
    headers: { Origin: origin, Cookie: cookie },
  });
  sockets.push(client.ws);
  client.ws.on("message", (raw) => {
    const frame = parseFrame(raw.toString());
    assert.ok(frame, `frame must satisfy the client contract: ${raw}`);
    client.frames.push(frame);
    if (frame.kind === "snapshot") client.snapshot = frame;
    if (frame.kind === "read")
      client.snapshot = { ...client.snapshot, unread: frame.unread };
  });
  await until(() => client.snapshot, "initial snapshot");
  return client;
}
async function send(client, text, room, kind = "notice") {
  const id = ++client.serial;
  client.ws.send(JSON.stringify({ id, room, text }));
  await until(
    () => client.frames.some((frame) => frame.id === id),
    `acknowledgement for ${text}`,
  );
  const reply = client.frames.find((frame) => frame.id === id);
  assert.equal(reply.kind, kind, `${text} -> ${reply.text}`);
  return reply.text;
}
const room = (client, name) =>
  client.snapshot.rooms.find((entry) => entry.name === name);
const provision = async (name, role) => {
  child.stdin.write(`/user ${name} ${name}-long-password ${role}\n`);
  await until(
    () => logs.includes(`Account ${name} created.`),
    `provision ${name}`,
  );
};

try {
  const data = join(temporary, "data");
  await mkdir(data);
  await writeFile(
    join(data, "config.json"),
    JSON.stringify({
      bind: "127.0.0.1:0",
      origins: [origin],
      production: false,
    }),
  );
  await start(data);
  await provision("alice", "admin");
  await provision("bob", "user");
  const alice = await connect(await login("alice", "alice-long-password"));
  const bob = await connect(await login("bob", "bob-long-password"));
  await send(alice, "/new lobby", null);
  await send(alice, "/add bob lobby");
  await until(() => room(bob, "lobby"), "membership delivery");

  // 1. Creation, role, and the optional-key form.
  const created = await send(
    alice,
    "/agent helper sk-smoke-secret-value",
    null,
  );
  assert.match(created, /Agent helper created with reply mode mention/);
  assert.equal(alice.snapshot.roles.helper, "agent");
  assert.equal(alice.snapshot.roles.alice, "admin");
  assert.equal(alice.snapshot.roles.bob, "user");
  await send(alice, "/agent helper", null, "error");
  await send(alice, "/agent bad/name sk-key", null, "error");

  // 2. The key never leaves the server.
  assert.ok(
    !JSON.stringify(alice.snapshot).includes("sk-smoke-secret-value"),
    "snapshots must not contain an agent key",
  );
  assert.ok(
    !JSON.stringify(
      await (
        await request("me", await login("bob", "bob-long-password"))
      ).json(),
    ).includes("sk-smoke-secret-value"),
    "another account must not receive an agent key",
  );
  assert.ok(
    !logs.includes("sk-smoke-secret-value"),
    "agent keys must not reach the log",
  );

  // 3. Invite an agent to a room with the ordinary /add command.
  await send(alice, "/add helper lobby");
  await until(
    () => room(alice, "lobby").agents.includes("helper"),
    "agent membership reaches the room view",
  );
  const users = await send(alice, "/users", null);
  assert.match(users, /helper — agent/);
  const members = await send(alice, "/members lobby", null);
  assert.match(members, /helper — agent/);

  // 4. Mention mode answers a mention and ignores everything else.
  await send(bob, "hello there", "lobby");
  await pause(400);
  assert.equal(
    room(alice, "lobby").messages.at(-1).text,
    "hello there",
    "mention mode must not answer an unmentioned message",
  );
  // An unusable key reports the provider status in the conversation without
  // echoing the credential.
  await send(bob, "@helper are you awake?", "lobby");
  await until(
    () =>
      room(alice, "lobby").messages.some(
        (message) => message.from === "helper",
      ),
    "an unusable key is reported",
  );
  const rejected = room(alice, "lobby").messages.at(-1);
  assert.equal(rejected.from, "helper");
  assert.match(
    rejected.text,
    /^⚠ The model provider rejected the request \(401\)/,
    `an invalid key is reported in the conversation, got: ${rejected.text}`,
  );
  assert.ok(
    !JSON.stringify(rejected).includes("sk-smoke-secret-value"),
    "the reported error must not echo the credential",
  );
  const agentCount = () =>
    room(alice, "lobby").messages.filter((message) => message.from === "helper")
      .length;
  const failedCount = agentCount();

  // With a usable provider key, a mention produces a real answer.
  const usableKey = liveKey || "sk-not-a-real-key";
  if (liveKey) await send(alice, `/agent-key ${liveKey} helper`);
  await send(bob, "@helper what is 2+2?", "lobby");
  await until(
    () => agentCount() > failedCount,
    "a mention produces an agent answer",
  );
  const answer = room(alice, "lobby").messages.at(-1);
  assert.equal(answer.from, "helper");
  assert.equal(answer.to, null);
  if (liveKey) {
    assert.doesNotMatch(
      answer.text,
      /^⚠/,
      `a live key must produce an answer, got: ${answer.text}`,
    );
  } else {
    assert.match(
      answer.text,
      /^⚠ The model provider rejected the request \(401\)/,
      `an invalid key is reported in the conversation, got: ${answer.text}`,
    );
  }
  assert.ok(
    !JSON.stringify(answer).includes(usableKey),
    "an agent message must not echo the credential",
  );

  // 4b. Web search is opt-in, needs its own key, and stays server-side.
  const searchKey = liveSearchKey || "tvly-dev-not-a-real-key";
  await send(alice, "/agent-search on", null, "error");
  const keyUpdate = await send(
    alice,
    "/agent-search-key tvly-dev-smoke helper",
  );
  assert.match(keyUpdate, /web search is off/, "a key alone does not search");
  await send(alice, "/agent-search on helper");
  assert.ok(
    !JSON.stringify(alice.snapshot).includes("tvly-dev-smoke"),
    "a search key never reaches a browser",
  );
  assert.ok(
    !logs.includes("tvly-dev-smoke"),
    "a search key never reaches the log",
  );
  await send(bob, "/agent-search off helper", null, "error");
  if (liveSearchKey) {
    // A question whose answer changes over time must be searched and cited.
    await send(alice, `/agent-search-key ${liveSearchKey} helper`);
    const before = agentCount();
    await send(bob, "@helper DeepSeek 最近发布了什么模型？请给出依据", "lobby");
    await until(() => agentCount() > before, "a searched answer");
    const searched = room(alice, "lobby").messages.at(-1).text;
    assert.doesNotMatch(searched, /^⚠/, `search fault: ${searched}`);
    assert.match(searched, /来源：/, "a searched answer cites its sources");
    assert.match(searched, /\(https?:\/\/[^\s)]+\)/, "sources are links");
    // A question that needs no search must not cite anything.
    const beforePlain = agentCount();
    await send(bob, "@helper 1+1 等于几？只回答数字", "lobby");
    await until(() => agentCount() > beforePlain, "a knowledge answer");
    const plain = room(alice, "lobby").messages.at(-1).text;
    assert.doesNotMatch(plain, /^⚠/, `knowledge fault: ${plain}`);
    assert.doesNotMatch(
      plain,
      /来源：/,
      `a question that needs no search must not search: ${plain}`,
    );
  } else {
    assert.equal(searchKey, "tvly-dev-not-a-real-key");
  }
  await send(alice, "/agent-search off helper");

  // 5. Auto mode answers an ordinary message, including a private message.
  await send(alice, "/agent-reply auto");
  const auto = await send(bob, "no mention needed now", "lobby");
  assert.equal(auto, "");
  const beforeAuto = agentCount();
  await until(
    () => agentCount() > beforeAuto,
    "auto mode answers without a mention",
  );
  await send(bob, "/tell helper hello in private", null);
  // A private conversation is visible only to its participants.
  await until(
    () =>
      bob.snapshot.direct.some(
        (message) => message.from === "helper" && message.to === "bob",
      ),
    "auto mode answers a private message",
  );
  assert.deepEqual(
    alice.snapshot.direct,
    [],
    "a private agent answer stays private",
  );
  assert.equal(
    bob.snapshot.unread["@direct:helper"].count > 0,
    true,
    "an agent answer is an ordinary unread private message",
  );

  // 6. Agent messages are ordinary chat messages, so people can react to them.
  const agentMessage = room(alice, "lobby").messages.at(-1);
  await send(bob, `/react ${agentMessage.id} 👍`, "lobby");
  await until(
    () =>
      room(alice, "lobby")
        .messages.find((message) => message.id === agentMessage.id)
        ?.reactions["👍"]?.includes("bob"),
    "a person may react to an agent message",
  );

  // 7. A rename carries membership, history, and the private conversation.
  await send(alice, "/agent-name assistant helper");
  assert.equal(alice.snapshot.roles.helper, undefined);
  assert.equal(alice.snapshot.roles.assistant, "agent");
  await until(
    () => room(alice, "lobby").members.includes("assistant"),
    "membership follows the rename",
  );
  assert.ok(
    !room(alice, "lobby").members.includes("helper"),
    "the old agent name leaves the room",
  );
  assert.ok(
    room(alice, "lobby").messages.some(
      (message) => message.from === "assistant",
    ),
    "retained agent messages are relabelled",
  );
  assert.ok(
    bob.snapshot.private_peers.includes("assistant"),
    "the private conversation moves to the new name",
  );
  assert.ok(
    !bob.snapshot.private_peers.includes("helper"),
    "the old private conversation key disappears",
  );
  await send(alice, "/agent-name assistant", null, "error");

  // 8. Non-owners cannot configure an agent, and agents cannot act as admins.
  await send(bob, "/agent-key sk-stolen assistant", null, "error");
  await send(bob, "/agent-reply mention assistant", null, "error");
  await send(bob, "/grant assistant", null, "error");
  await send(alice, "/grant assistant", null, "error");
  assert.ok(
    !logs.includes("sk-stolen"),
    "a rejected key change must not reach the log",
  );

  // 9. Disabling an agent stops its answers and revokes membership.
  await send(alice, "/disable assistant");
  await until(
    () => !room(alice, "lobby").members.includes("assistant"),
    "a disabled agent loses membership",
  );
  const before = room(alice, "lobby").messages.length;
  await send(bob, "anyone home?", "lobby");
  await pause(400);
  assert.equal(
    room(alice, "lobby").messages.length,
    before + 1,
    "a disabled agent does not answer",
  );
  await send(alice, "/enable assistant");
  await send(alice, "/add assistant lobby");

  // 10. Removal deletes the agent and its private conversations.
  await send(alice, "/agent-remove assistant");
  assert.equal(alice.snapshot.roles.assistant, undefined);
  await until(
    () => !bob.snapshot.private_peers.includes("assistant"),
    "removal deletes the agents private conversations",
  );
  await send(alice, "/agent-key sk-after-remove assistant", null, "error");

  // 11. Agent state survives a restart.
  await send(alice, "/agent keeper sk-restart-secret", null);
  await send(alice, "/agent-reply auto keeper");
  await send(alice, "/add keeper lobby");
  await stop();
  await start(data);
  const resumed = await connect(await login("alice", "alice-long-password"));
  assert.equal(resumed.snapshot.roles.keeper, "agent");
  assert.ok(
    room(resumed, "lobby").agents.includes("keeper"),
    "an agent's membership survives a restart",
  );
  assert.ok(
    !JSON.stringify(resumed.snapshot).includes("sk-restart-secret"),
    "a restart does not expose the stored key",
  );
  await send(resumed, "/agent-name reviewer keeper");
  assert.equal(resumed.snapshot.roles.reviewer, "agent");
  await send(resumed, "/agent-remove reviewer");

  console.log(
    `PASS: agent roles, /agent creation, /add invites, mention and auto replies, private answers, renames, permission boundaries, disabling, removal, restart persistence, and credential redaction${liveKey ? " with a live provider round trip" : ""}${liveSearchKey ? " and a live web search" : ""}.`,
  );
} finally {
  try {
    await stop();
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
