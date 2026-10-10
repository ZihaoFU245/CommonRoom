import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, cp, rm } from "node:fs/promises";
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
const temporary = await mkdtemp(join(tmpdir(), "commonroom-smoke-"));
const origin = "http://localhost:5173";
const publicOrigin = "https://chat.example.com";
let production = false;
const sockets = [];
let child;
let base;
let logs = "";
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(predicate, label) {
  const start = Date.now();
  while (!predicate()) {
    if (Date.now() - start > 10000)
      throw new Error(`Timed out: ${label}\n${logs}`);
    await pause(30);
  }
}
async function start(data, release = false, trust, baseUrl) {
  logs = "";
  production = release;
  const env = { ...process.env, CHAT_DATA: data, RUST_LOG: "chat=info" };
  delete env.CHAT_PRODUCTION;
  delete env.CHAT_ORIGIN;
  delete env.CHAT_BIND;
  delete env.CHAT_TRUST;
  delete env.CHAT_BASE_URL;
  if (baseUrl) env.CHAT_BASE_URL = baseUrl;
  if (trust) env.CHAT_TRUST = trust;
  if (release) {
    env.CHAT_ORIGIN = publicOrigin;
    env.CHAT_BIND = "127.0.0.1:0";
  }
  child = spawn(
    release
      ? join(root, process.platform === "win32" ? "chat.exe" : "chat")
      : binary,
    [],
    {
      cwd: release ? temporary : root,
      env,
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  let failure;
  child.on("error", (error) => {
    failure = error;
  });
  child.stdout.on("data", (data) => {
    logs += data.toString();
  });
  child.stderr.on("data", (data) => {
    logs += data.toString();
  });
  await until(
    () => failure || /Chat server ready/.test(logs) || child.exitCode !== null,
    "server startup",
  );
  if (failure) throw failure;
  assert.equal(child.exitCode, null, logs);
  const match = logs.match(/address=127\.0\.0\.1:(\d+)/);
  assert.ok(match, logs);
  base = `http://127.0.0.1:${match[1]}${(baseUrl || "/").replace(/\/$/, "")}`;
}
async function stop() {
  for (const socket of sockets.splice(0)) socket.ws.terminate();
  if (!child || child.exitCode !== null) return;
  const current = child;
  current.kill("SIGTERM");
  await until(
    () => current.exitCode !== null || current.signalCode !== null,
    "graceful shutdown",
  );
  assert.equal(current.exitCode, 0, logs);
  child = null;
}
async function request(
  path,
  cookie,
  body,
  requestOrigin = production ? publicOrigin : origin,
  protocol = production ? "https" : null,
) {
  return fetch(`${base}/api/${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      Origin: requestOrigin,
      ...(protocol ? { "X-Forwarded-Proto": protocol } : {}),
      ...(cookie ? { Cookie: cookie } : {}),
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
}
async function login(username, password = `${username}-long-password`) {
  const response = await request("login", null, {
    username,
    password,
  });
  assert.equal(response.status, 200, await response.clone().text());
  const cookie = response.headers.get("set-cookie");
  assert.match(cookie, /HttpOnly/);
  return cookie.split(";")[0];
}
async function connect(cookie) {
  const client = {
    ws: new WebSocket(`${base.replace("http:", "ws:")}/ws`, {
      headers: {
        Origin: production ? publicOrigin : origin,
        Cookie: cookie,
        ...(production ? { "X-Forwarded-Proto": "https" } : {}),
      },
    }),
    frames: [],
    serial: 0,
    snapshot: null,
  };
  sockets.push(client);
  client.ws.on("message", (raw) => {
    const frame = parseFrame(raw.toString());
    assert.ok(
      frame,
      "Server frame must satisfy the TypeScript client's runtime contract",
    );
    client.frames.push(frame);
    if (frame.kind === "snapshot") client.snapshot = frame;
    if (frame.kind === "read")
      client.snapshot = { ...client.snapshot, unread: frame.unread };
  });
  let error;
  client.ws.on("error", (e) => {
    error = e;
  });
  await until(() => error || client.snapshot, "initial WebSocket snapshot");
  if (error) throw error;
  return client;
}
async function send(
  client,
  text,
  room = null,
  kind = "notice",
  escaped = false,
) {
  const id = ++client.serial;
  const frame = JSON.stringify({ id, room, text });
  client.ws.send(escaped ? frame.replaceAll("🙂", "\\ud83d\\ude42") : frame);
  await until(
    () => client.frames.some((f) => f.id === id),
    `acknowledgement for ${text}`,
  );
  const reply = client.frames.find((f) => f.id === id);
  assert.equal(reply.kind, kind, reply.text);
}

try {
  // Port zero avoids collisions. This setting persists through migration.
  const { mkdir, readFile, writeFile } = await import("node:fs/promises");
  const source = join(temporary, "original");
  await mkdir(source);
  await writeFile(
    join(source, "config.json"),
    JSON.stringify({
      bind: "127.0.0.1:0",
      origins: [origin],
      production: false,
      max_users: 4,
      max_rooms: 1,
    }),
  );
  await start(source);
  for (const [name, role] of [
    ["alice", "admin"],
    ["bob", "user"],
    ["eve", "user"],
  ]) {
    child.stdin.write(`/user ${name} ${name}-long-password ${role}\n`);
    await until(
      () => logs.includes(`Account ${name} created.`),
      `provision ${name}`,
    );
  }
  child.stdin.write(
    "/grant alice @global x:account.password.reset\n/grant alice @global /reset\n",
  );
  await until(
    () => logs.includes("Grant updated."),
    "explicit reset delegation",
  );
  assert.equal((await request("me")).status, 401);
  assert.equal(
    (
      await request("login", null, {
        username: "alice",
        password: "wrong-password",
      })
    ).status,
    401,
  );
  assert.equal(
    (
      await request(
        "login",
        null,
        { username: "alice", password: "alice-long-password" },
        "https://evil.example",
      )
    ).status,
    403,
  );
  const cookies = {};
  for (const name of ["alice", "bob", "eve"]) cookies[name] = await login(name);
  const alice = await connect(cookies.alice),
    bob = await connect(cookies.bob),
    eve = await connect(cookies.eve);
  // Rename must preserve both live sockets and the original session cookies.
  const renameTab = await connect(cookies.bob);
  await send(bob, "/rename 测试用户🙂");
  await until(
    () =>
      bob.snapshot.username === "测试用户🙂" &&
      renameTab.snapshot.username === "测试用户🙂",
    "rename delivered to both authenticated sockets",
  );
  assert.equal(
    (await (await request("me", cookies.bob)).json()).username,
    "测试用户🙂",
  );
  await send(renameTab, "/whoami");
  assert.match(
    renameTab.frames.find((frame) => frame.id === renameTab.serial).text,
    /Name: 测试用户🙂/,
  );
  await send(renameTab, "/rename bob");
  await until(
    () =>
      bob.snapshot.username === "bob" && renameTab.snapshot.username === "bob",
    "rename back preserves socket identity",
  );
  renameTab.ws.close();
  await until(
    () => renameTab.ws.readyState === WebSocket.CLOSED,
    "rename test tab closed",
  );
  await send(bob, "/whoami");
  assert.match(
    bob.frames.find((f) => f.id === bob.serial).text,
    /Permission: user/,
  );
  assert.ok(
    bob.snapshot.commands.some((command) => command.name === "/console"),
  );
  await send(bob, "/console");
  assert.equal(
    bob.frames.find((f) => f.id === bob.serial).text,
    "Command view opened.",
  );
  await send(bob, "/console extra", null, "error");
  await send(bob, "/grant bob", null, "error");
  await send(bob, "/configs", null, "error");
  await send(bob, "/clean 7d @all", null, "error");
  const sudoTab = await connect(cookies.bob);
  await send(sudoTab, "/sudo /configs", null, "error");
  child.stdin.write("/grant bob @global /sudo\n");
  await until(
    () => sudoTab.snapshot.commands.some((command) => command.name === "/sudo"),
    "sudo command grant delivery",
  );
  await send(sudoTab, "/sudo /configs", null, "error");
  child.stdin.write("/grant bob @global x:command.sudo\n");
  await until(
    () => sudoTab.snapshot.permissions.includes("x:command.sudo"),
    "sudo action grant delivery",
  );
  await send(sudoTab, "/sudo /configs");
  await send(sudoTab, "/sudo /whoami");
  assert.match(
    sudoTab.frames.find((frame) => frame.id === sudoTab.serial).text,
    /Name: bob\nPermission: su/,
  );
  assert.equal(sudoTab.snapshot.admin, false);
  assert.deepEqual(sudoTab.snapshot.groups, ["user"]);
  await send(sudoTab, "/configs", null, "error");
  await send(sudoTab, "/sudo /sudo /configs", null, "error");
  child.stdin.write("/revoke bob @global x:command.sudo\n");
  await until(
    () => !sudoTab.snapshot.permissions.includes("x:command.sudo"),
    "sudo action revocation delivery",
  );
  await send(sudoTab, "/sudo /configs", null, "error");
  child.stdin.write("/revoke bob @global /sudo\n");
  await until(
    () =>
      !sudoTab.snapshot.commands.some((command) => command.name === "/sudo"),
    "sudo command revocation delivery",
  );
  sudoTab.ws.close();
  await until(
    () => sudoTab.ws.readyState === WebSocket.CLOSED,
    "sudo test tab closes",
  );
  await send(alice, "/configs", null);
  const configs = JSON.parse(
    alice.frames.find((f) => f.id === alice.serial).text,
  );
  assert.equal(configs.max_users, 4);
  assert.equal(configs.max_rooms, 1);
  assert.equal(configs.max_messages, 1000);
  await send(alice, "/configs max_users 9", null, "error");
  await send(alice, "/help");
  const help = alice.frames.find((f) => f.id === alice.serial).text;
  assert.ok(help.split("\n").length > 10);
  assert.ok(help.includes("/reset"));
  await until(
    () => alice.snapshot.online.includes("bob"),
    "online presence broadcast",
  );
  const presenceTab = await connect(cookies.bob);
  assert.equal(
    presenceTab.snapshot.online.filter((name) => name === "bob").length,
    1,
  );
  presenceTab.ws.close();
  await until(
    () => presenceTab.ws.readyState === WebSocket.CLOSED,
    "second tab closes",
  );
  assert.ok(
    (await (await request("me", cookies.alice)).json()).online.includes("bob"),
  );
  assert.deepEqual(bob.snapshot.rooms, [], "new users start without rooms");
  assert.deepEqual(
    bob.snapshot.private_peers,
    [],
    "unmessaged accounts are not private contacts",
  );
  await send(bob, "/user denied abc", null, "error");
  await send(bob, "/reset alice abc", null, "error");
  await send(bob, "/disable alice", null, "error");
  await send(bob, "/enable alice", null, "error");
  await send(alice, "/user carol abc", null);
  await send(alice, "/user dave abc", null, "error");
  child.stdin.write("/user dave abc\n");
  await until(
    () => logs.includes("User limit reached (4)."),
    "stdin user limit",
  );
  const carolCookie = await login("carol", "abc");
  const carol = await connect(carolCookie);
  assert.deepEqual(carol.snapshot.rooms, []);
  await until(
    () => alice.snapshot.online.includes("carol"),
    "new account online",
  );
  await send(alice, "/reset carol xyz", null);
  await until(
    () => carol.ws.readyState === WebSocket.CLOSED,
    "admin password reset revokes sessions",
  );
  await login("carol", "xyz");
  const carol2 = await connect(await login("carol", "xyz"));
  const carol3Cookie = await login("carol", "xyz");
  const carol3 = await connect(carol3Cookie);
  await send(carol2, "/passwd wrong abc", null, "error");
  await send(carol2, "/passwd xyz abc", null);
  await until(
    () => carol3.ws.readyState === WebSocket.CLOSED,
    "password change revokes other sessions",
  );
  assert.equal((await request("me", carol3Cookie)).status, 401);
  await send(carol2, "/whoami", null);
  await send(alice, "/disable carol", null);
  await send(alice, "/enable carol", null);
  await send(alice, "/disable alice", null, "error");
  await send(alice, "/grant bob");
  await until(() => bob.snapshot.admin, "live admin grant");
  await send(bob, "/whoami");
  assert.match(
    bob.frames.find((f) => f.id === bob.serial).text,
    /Permission: admin/,
  );
  await send(alice, "/revoke bob");
  await until(() => !bob.snapshot.admin, "live admin revoke");
  assert.ok(
    !eve.frames.some((f) => f.kind === "notice" || f.kind === "error"),
    "command replies must not reach other clients",
  );
  assert.equal(
    alice.snapshot.rooms.flatMap((r) => r.messages).length,
    0,
    "commands must not enter persisted chat history",
  );
  await send(bob, "/new forbidden", null, "error");
  await send(alice, "/new study");
  await send(alice, "/new overflow", null, "error");
  child.stdin.write("/new overflow\n");
  await until(
    () => logs.includes("Room limit reached (1)."),
    "stdin room limit",
  );
  await send(bob, "/join study", null, "error");
  await send(alice, "/add bob study");
  await until(
    () => bob.snapshot.rooms.some((r) => r.name === "study"),
    "membership delivery",
  );
  await send(bob, "a retained room message", "study");
  const multilingual = "你好 日本語 한국어 مرحبا नमस्ते Привет שלום 🙂 e\u0301";
  await send(bob, multilingual, "study");
  await until(
    () =>
      bob.snapshot.rooms.find((r) => r.name === "study").messages.at(-1)
        ?.text === multilingual,
    "Unicode room delivery",
  );
  const roomMessageId = bob.snapshot.rooms
    .find((r) => r.name === "study")
    .messages.at(-1).id;
  await send(alice, `/react ${roomMessageId} 好👍`, "study");
  await until(
    () =>
      bob.snapshot.rooms
        .find((r) => r.name === "study")
        .messages.find((m) => m.id === roomMessageId)
        ?.reactions["好👍"]?.includes("alice"),
    "reaction broadcast",
  );
  await send(bob, `/react ${roomMessageId} 好👍`, "study");
  await until(
    () =>
      alice.snapshot.rooms
        .find((r) => r.name === "study")
        .messages.find((m) => m.id === roomMessageId)?.reactions["好👍"]
        ?.length === 2,
    "shared reaction count",
  );
  await send(alice, `/react ${roomMessageId} 好👍`, "study");
  await send(bob, `/reply ${roomMessageId} @alice 回答 🙂`, "study");
  await until(
    () =>
      alice.snapshot.rooms.find((r) => r.name === "study").messages.at(-1)
        ?.reply?.id === roomMessageId,
    "reply delivery",
  );
  const roomReply = alice.snapshot.rooms
    .find((r) => r.name === "study")
    .messages.at(-1);
  assert.deepEqual(roomReply.mentions, ["alice"]);
  assert.equal(roomReply.reply.text, multilingual);
  await send(eve, `/react ${roomMessageId} 👀`, "study", "error");
  await send(eve, `/reply ${roomMessageId} no access`, "study", "error");
  await send(alice, `/retract ${roomMessageId}`, "study", "error");
  await send(eve, `/retract ${roomMessageId}`, "study", "error");
  await send(bob, "temporary room deletion fixture", "study");
  await until(
    () =>
      alice.snapshot.rooms.find((r) => r.name === "study").messages.at(-1)
        ?.text === "temporary room deletion fixture",
    "room deletion fixture",
  );
  const retractRoomId = alice.snapshot.rooms
    .find((r) => r.name === "study")
    .messages.at(-1).id;
  await send(bob, `/retract ${retractRoomId}`, "study");
  await until(
    () =>
      !alice.snapshot.rooms
        .find((r) => r.name === "study")
        .messages.some((m) => m.id === retractRoomId),
    "room deletion broadcast",
  );
  const longUnicode = "🙂".repeat(4000);
  await send(bob, `/tell alice ${longUnicode}`, null, "notice", true);
  await until(
    () => alice.snapshot.direct.some((m) => m.text === longUnicode),
    "full-length UTF-8 private message",
  );
  await send(bob, "/tell alice a private message");
  await until(
    () => alice.snapshot.direct.some((m) => m.text === "a private message"),
    "private message delivery",
  );
  assert.ok(
    !eve.frames.some((frame) =>
      frame.direct?.some((message) => message.text === "a private message"),
    ),
  );
  assert.ok(
    !eve.frames.some((frame) =>
      frame.rooms?.some((room) =>
        room.messages.some(
          (message) => message.text === "a retained room message",
        ),
      ),
    ),
  );
  const dmId = alice.snapshot.direct.find(
    (m) => m.text === "a private message",
  ).id;
  await send(alice, `/react ${dmId} ❤️`, null);
  await send(alice, `/reply ${dmId} @bob 私信`, null);
  await until(
    () => bob.snapshot.direct.at(-1)?.reply?.id === dmId,
    "private reply delivery",
  );
  assert.equal(bob.snapshot.direct.at(-1).to, "bob");
  assert.deepEqual(bob.snapshot.direct.at(-1).mentions, ["bob"]);
  await send(eve, `/react ${dmId} ❤️`, null, "error");
  await send(eve, `/reply ${dmId} leak`, null, "error");
  await send(alice, `/retract ${dmId}`, null, "error");
  await send(eve, `/retract ${dmId}`, null, "error");
  await send(bob, "/tell alice temporary private deletion fixture", null);
  await until(
    () =>
      alice.snapshot.direct.at(-1)?.text ===
      "temporary private deletion fixture",
    "private deletion fixture",
  );
  const retractPrivateId = alice.snapshot.direct.at(-1).id;
  await send(bob, `/retract ${retractPrivateId}`, null);
  await until(
    () => !alice.snapshot.direct.some((m) => m.id === retractPrivateId),
    "private deletion broadcast",
  );
  const bobOtherDevice = await connect(cookies.bob);
  const readState = bob.snapshot.unread["@direct:alice"];
  assert.ok(readState.count > 0);
  const retainedDM = await (
    await request("history?view=%40direct%3Aalice", cookies.bob)
  ).json();
  assert.ok(retainedDM.messages.length >= readState.count);
  assert.equal(
    (await request("history?view=study", cookies.carol)).status,
    401,
  );
  assert.equal(
    (await request("history?view=%40direct%3Abob", cookies.eve)).status,
    403,
  );
  assert.equal(
    (
      await request("read", cookies.eve, {
        view: "study",
        through: readState.through,
      })
    ).status,
    400,
  );
  assert.equal(
    (
      await request("read", cookies.bob, {
        view: "@direct:alice",
        through: readState.through + 999,
      })
    ).status,
    400,
  );
  const aliceFrameCount = alice.frames.length;
  assert.equal(
    (
      await request("read", cookies.bob, {
        view: "@direct:alice",
        through: readState.through,
      })
    ).status,
    200,
  );
  await until(
    () => bobOtherDevice.snapshot.unread["@direct:alice"].count === 0,
    "cross-device read position",
  );
  await until(
    () => bob.snapshot.unread["@direct:alice"].count === 0,
    "original device read position",
  );
  assert.ok(
    bobOtherDevice.frames.some(
      (frame) => frame.kind === "read" && !frame.rooms && !frame.direct,
    ),
    "read updates contain metadata only",
  );
  assert.equal(
    alice.frames.length,
    aliceFrameCount,
    "read changes are sent only to this user's devices",
  );
  await send(alice, "/kick bob study");
  await until(
    () => !bob.snapshot.rooms.some((r) => r.name === "study"),
    "membership revocation",
  );
  await send(bob, "not allowed", "study", "error");
  await send(alice, "/clean 7d study", null);
  await send(alice, "/add bob study");
  await stop();
  const destination = join(temporary, "migrated");
  await cp(source, destination, { recursive: true });
  const migratedConfigPath = join(destination, "config.json");
  const migratedConfig = JSON.parse(await readFile(migratedConfigPath, "utf8"));
  migratedConfig.max_messages = 4;
  await writeFile(migratedConfigPath, JSON.stringify(migratedConfig));
  await start(destination);
  const resumed = await request("me", cookies.alice);
  assert.equal(resumed.status, 200);
  const state = await resumed.json();
  assert.equal(state.admin, true);
  assert.ok(
    state.rooms
      .find((r) => r.name === "study")
      .messages.some((m) => m.text === "a retained room message"),
  );
  assert.ok(state.direct.some((m) => m.text === "a private message"));
  assert.ok(
    state.rooms
      .find((r) => r.name === "study")
      .messages.some(
        (m) => m.reply?.text === multilingual && m.mentions.includes("alice"),
      ),
  );
  assert.ok(state.direct.some((m) => m.reply && m.mentions.includes("bob")));
  assert.ok(
    state.direct
      .find((m) => m.text === "a private message")
      .reactions["❤️"].includes("alice"),
  );
  const migratedAlice = await connect(cookies.alice);
  await send(migratedAlice, "/configs", null);
  assert.equal(
    JSON.parse(
      migratedAlice.frames.find((f) => f.id === migratedAlice.serial).text,
    ).max_messages,
    4,
  );
  for (const text of [
    "ring first",
    "ring oldest",
    "ring next",
    "ring middle 你好",
    "ring newest 🙂",
  ]) {
    await send(migratedAlice, text, "study");
  }
  await until(
    () =>
      migratedAlice.snapshot.rooms
        .find((r) => r.name === "study")
        .messages.at(-1)?.text === "ring newest 🙂",
    "bounded room delivery",
  );
  assert.deepEqual(
    migratedAlice.snapshot.rooms
      .find((r) => r.name === "study")
      .messages.map((m) => m.text),
    ["ring oldest", "ring next", "ring middle 你好", "ring newest 🙂"],
  );
  await send(migratedAlice, "/history 5", "study", "error");
  await send(migratedAlice, "/history", "study");
  assert.match(
    migratedAlice.frames.find((f) => f.id === migratedAlice.serial).text,
    /ring middle 你好/,
  );
  const boundedState = await (await request("me", cookies.alice)).json();
  assert.equal(
    boundedState.rooms.find((r) => r.name === "study").messages.length,
    4,
  );
  assert.ok(boundedState.direct.some((m) => m.text === "a private message"));
  assert.equal(
    (await (await request("me", cookies.bob)).json()).unread["@direct:alice"]
      .count,
    0,
    "read position survives moving data folder",
  );
  for (let i = 0; i < 5; i++)
    await send(migratedAlice, `/tell bob bounded ${i} 你好`);
  await send(migratedAlice, "/tell eve independent private history");
  const boundedDM = await (
    await request("history?view=%40direct%3Abob", cookies.alice)
  ).json();
  assert.deepEqual(
    boundedDM.messages.map((m) => m.text),
    ["bounded 1 你好", "bounded 2 你好", "bounded 3 你好", "bounded 4 你好"],
  );
  const independentDM = await (
    await request("history?view=%40direct%3Aeve", cookies.alice)
  ).json();
  assert.equal(
    independentDM.messages.at(-1).text,
    "independent private history",
  );
  const migratedBob = await connect(cookies.bob);
  child.stdin.write("/reset bob changed-long-password\n");
  await until(
    () => logs.includes("Account bob password reset."),
    "password reset",
  );
  await until(
    () => migratedBob.ws.readyState === WebSocket.CLOSED,
    "reset disconnects old socket",
  );
  assert.equal((await request("me", cookies.bob)).status, 401);
  child.stdin.write("/disable eve\n");
  await until(() => logs.includes("Disabled eve."), "disable account");
  assert.equal((await request("me", cookies.eve)).status, 401);
  assert.equal((await request("logout", cookies.alice, {})).status, 200);
  assert.equal((await request("me", cookies.alice)).status, 401);
  assert.ok(
    !logs.includes("changed-long-password"),
    "passwords must not appear in logs",
  );
  await stop();
  // A second device must receive the complete directory before it sends any
  // command, including empty joined rooms and peers outside visible history.
  const devices = join(temporary, "devices");
  await mkdir(devices);
  await writeFile(
    join(devices, "config.json"),
    JSON.stringify({
      bind: "127.0.0.1:0",
      origins: [origin],
      production: false,
    }),
  );
  await start(devices);
  for (const name of ["alice", "bob", "eve"]) {
    child.stdin.write(
      `/user ${name} ${name}-long-password ${name === "alice" ? "admin" : "user"}\n`,
    );
    await until(
      () => logs.includes(`Account ${name} created.`),
      `device account ${name}`,
    );
  }
  const deviceCookie = await login("alice");
  const deviceOne = await connect(deviceCookie);
  for (const name of ["empty", "quiet", "active"])
    await send(deviceOne, `/new ${name}`, null);
  await send(deviceOne, "/tell bob older conversation", null);
  // Busy Eve history must not evict Bob or his separate snapshot tail.
  for (let i = 0; i < 55; i++) {
    await send(deviceOne, `/tell eve recent ${i}`, null);
    if (i % 20 === 19) await pause(10100); // honor the WebSocket rate limit
  }
  const eveDeviceCookie = await login("eve");
  const eveDevice = await connect(eveDeviceCookie);
  assert.equal(eveDevice.snapshot.unread["@direct:alice"].count, 55);
  assert.equal(eveDevice.snapshot.direct.length, 50);
  const eveHistory = await (
    await request("history?view=%40direct%3Aalice", eveDeviceCookie)
  ).json();
  assert.equal(eveHistory.messages.length, 55);
  assert.equal(
    eveHistory.messages[0].sequence,
    eveDevice.snapshot.unread["@direct:alice"].first,
  );
  assert.equal(
    (
      await request("read", eveDeviceCookie, {
        view: "@direct:alice",
        through: eveHistory.messages[4].sequence,
      })
    ).status,
    200,
  );
  await until(
    () => eveDevice.snapshot.unread["@direct:alice"].count === 50,
    "partial history read",
  );
  const assertDirectory = (snapshot) => {
    assert.deepEqual(
      snapshot.rooms.map((room) => room.name),
      ["active", "empty", "quiet"],
    );
    assert.ok(snapshot.rooms.every((room) => room.messages.length === 0));
    assert.deepEqual(snapshot.private_peers, ["bob", "eve"]);
    assert.ok(snapshot.direct.some((message) => message.to === "bob"));
  };
  const secondCookie = await login("alice");
  assertDirectory(await (await request("me", secondCookie)).json());
  const deviceTwo = await connect(secondCookie);
  assertDirectory(deviceTwo.snapshot);
  await send(deviceOne, "/new added_elsewhere", null);
  await until(
    () =>
      deviceTwo.snapshot.rooms.some((room) => room.name === "added_elsewhere"),
    "other device membership update",
  );
  deviceTwo.ws.terminate();
  const reconnected = await connect(secondCookie);
  assert.deepEqual(
    reconnected.snapshot.rooms.map((room) => room.name),
    ["active", "added_elsewhere", "empty", "quiet"],
  );
  assert.deepEqual(reconnected.snapshot.private_peers, ["bob", "eve"]);
  // Agents: role, invitation, a mention-driven answer, and credential handling.
  // scripts/agent-check.mjs covers auto mode, renames, removal, and disabling.
  await send(deviceOne, "/new agent-room", null);
  await send(deviceOne, "/agent helper sk-invalid-smoke-key", null);
  assert.equal(
    deviceOne.snapshot.roles.helper,
    "agent",
    "an agent appears in the directory with the agent role",
  );
  // The value-only form configures the one agent this account owns.
  await send(deviceOne, "/agent-reply auto helper");
  await send(deviceOne, "/agent-reply mention");
  assert.ok(
    deviceOne.snapshot.commands.some(
      (command) => command.name === "/agent-reply",
    ),
    "an agent owner is offered agent configuration commands",
  );
  await send(deviceOne, "/add helper agent-room");
  await until(
    () =>
      deviceOne.snapshot.rooms
        .find((room) => room.name === "agent-room")
        .agents.includes("helper"),
    "rooms report which members are agents",
  );
  assert.equal(
    deviceOne.snapshot.online.includes("helper"),
    false,
    "agents hold no sessions and never appear online",
  );
  await send(deviceOne, "@helper are you there?", "agent-room");
  await until(
    () =>
      deviceOne.snapshot.rooms
        .find((room) => room.name === "agent-room")
        .messages.some((message) => message.from === "helper"),
    "a mention produces an agent message",
  );
  // An unusable provider key is reported in the conversation, and the key
  // never reaches a browser or the server log.
  const agentFailure = deviceOne.snapshot.rooms
    .find((room) => room.name === "agent-room")
    .messages.at(-1);
  assert.equal(agentFailure.from, "helper");
  assert.match(
    agentFailure.text,
    /^⚠ The model provider rejected|^⚠ The model provider could not/,
  );
  assert.ok(
    !JSON.stringify(deviceOne.snapshot).includes("sk-invalid-smoke-key"),
    "agent keys never reach a browser snapshot",
  );
  assert.ok(
    !logs.includes("sk-invalid-smoke-key"),
    "agent keys never appear in server logs",
  );
  // A regular account may create its own agent but never configure another.
  const agentCookie = await login("eve");
  const eveAgentDevice = await connect(agentCookie);
  await send(eveAgentDevice, "/agent eve-helper sk-invalid-eve-key", null);
  await until(
    () => eveAgentDevice.snapshot.roles["eve-helper"] === "agent",
    "a regular account may create an agent",
  );
  await send(eveAgentDevice, "/agent-key sk-stolen helper", null, "error");
  assert.ok(
    !logs.includes("sk-stolen"),
    "a non-owner never changes another agent's key",
  );
  await send(deviceOne, "/agent-remove helper");
  await until(
    () => deviceOne.snapshot.roles.helper === undefined,
    "an agent can be removed",
  );
  await send(eveAgentDevice, "/agent-remove helper", null, "error");
  await send(deviceOne, "/agent-remove eve-helper");
  for (let i = 0; i < 2; i++) await pause(10100); // honor the WebSocket rate limit
  await send(deviceOne, "/delete agent-room");
  // Delete accounts through both command entry points. Existing sessions and
  // private histories must not become accessible to a replacement username.
  const bobCookie = await login("bob");
  const bobSecondCookie = await login("bob");
  const bobDevice = await connect(bobCookie);
  const bobSecondDevice = await connect(bobSecondCookie);
  await send(eveDevice, "/deleteuser bob", null, "error");
  await send(deviceOne, "/deleteuser alice", null, "error");
  await send(deviceOne, "/add bob active", null);
  await send(bobDevice, "preserved after deletion", "active");
  await until(
    () =>
      deviceOne.snapshot.rooms.find((r) => r.name === "active").messages
        .length === 1,
    "original deleted-user message",
  );
  const originalId = deviceOne.snapshot.rooms.find((r) => r.name === "active")
    .messages[0].id;
  await send(deviceOne, `/reply ${originalId} @bob preserved reply`, "active");
  await send(bobDevice, `/react ${originalId} 👍`, "active");
  await send(deviceOne, "/tell bob private before deletion", null);
  await send(deviceOne, "/deleteuser bob", null);
  await until(
    () =>
      bobDevice.ws.readyState === WebSocket.CLOSED &&
      bobSecondDevice.ws.readyState === WebSocket.CLOSED,
    "deleted-user sessions disconnect",
  );
  assert.equal((await request("me", bobCookie)).status, 401);
  assert.equal((await request("me", bobSecondCookie)).status, 401);
  await until(
    () =>
      !reconnected.snapshot.users.includes("bob") &&
      !reconnected.snapshot.private_peers.includes("bob"),
    "other admin device sees deletion",
  );
  const retainedRoom = reconnected.snapshot.rooms.find(
    (r) => r.name === "active",
  );
  assert.ok(!retainedRoom.members.includes("bob"));
  assert.equal(retainedRoom.messages[0].from, "bob (deleted)");
  assert.equal(retainedRoom.messages[0].id, originalId);
  assert.deepEqual(retainedRoom.messages[0].reactions, {});
  assert.equal(retainedRoom.messages[1].reply.from, "bob (deleted)");
  assert.deepEqual(retainedRoom.messages[1].mentions, []);
  assert.equal(
    (await request("history?view=%40direct%3Abob", deviceCookie)).status,
    403,
  );
  await send(deviceOne, "/user bob fresh-password", null);
  const freshBobCookie = await login("bob", "fresh-password");
  const freshBob = await connect(freshBobCookie);
  assert.deepEqual(freshBob.snapshot.rooms, []);
  assert.deepEqual(freshBob.snapshot.private_peers, []);
  assert.deepEqual(freshBob.snapshot.unread, {});
  await send(deviceOne, "/tell bob fresh private", null);
  await until(
    () => freshBob.snapshot.direct.length === 1,
    "replacement user's fresh private history",
  );
  assert.equal(freshBob.snapshot.direct[0].text, "fresh private");
  child.stdin.write("/deleteuser eve\n");
  await until(
    () => logs.includes("Deleted account eve"),
    "stdin account deletion",
  );
  await until(
    () => eveDevice.ws.readyState === WebSocket.CLOSED,
    "stdin deletion revokes live session",
  );
  assert.equal((await request("me", eveDeviceCookie)).status, 401);
  await stop();
  await start(devices);
  const restartedDirectory = await (await request("me", deviceCookie)).json();
  assert.ok(!restartedDirectory.users.includes("eve"));
  assert.deepEqual(restartedDirectory.private_peers, ["bob"]);
  assert.equal(
    restartedDirectory.rooms.find((r) => r.name === "active").messages[0].from,
    "bob (deleted)",
  );
  await stop();
  const trustData = join(temporary, "trust");
  await mkdir(trustData);
  await writeFile(
    join(trustData, "config.json"),
    JSON.stringify({
      bind: "127.0.0.1:0",
      origins: [origin],
      production: false,
    }),
  );
  await start(trustData);
  for (const [name, role] of [
    ["owner", "admin"],
    ["reader", "user"],
    ["other", "admin"],
  ]) {
    child.stdin.write(`/user ${name} ${name}-long-password ${role}\n`);
    await until(
      () => logs.includes(`Account ${name} created.`),
      `trust account ${name}`,
    );
  }
  const ownerCookie = await login("owner"),
    readerCookie = await login("reader"),
    otherCookie = await login("other");
  const ownerClient = await connect(ownerCookie),
    readerClient = await connect(readerCookie),
    otherClient = await connect(otherCookie);
  child.stdin.write(
    "/grant reader @global w:room.create\n/grant reader @global /new\n",
  );
  await until(
    () =>
      readerClient.snapshot.permissions.includes("w:room.create") &&
      readerClient.snapshot.commands.some((command) => command.name === "/new"),
    "user receives room creation grants",
  );
  await send(readerClient, "/new delegated", null);
  await send(readerClient, "/grant owner delegated /add", "delegated");
  await send(ownerClient, "/add other delegated", null, "error");
  await send(readerClient, "/grant owner delegated x:member.add", "delegated");
  await send(ownerClient, "/add other delegated", null, "error");
  await send(readerClient, "/add owner", "delegated");
  await send(ownerClient, "/add other", "delegated");
  await until(
    () => otherClient.snapshot.rooms.some((room) => room.name === "delegated"),
    "delegated invitation grants participant access",
  );
  await send(otherClient, "invited participant", "delegated");
  await send(readerClient, "/revoke owner delegated x:member.add", "delegated");
  await send(ownerClient, "/add other", "delegated", "error");
  await send(readerClient, "/revoke owner delegated /add", "delegated");
  await send(ownerClient, "/add other", "delegated", "error");
  await send(readerClient, "/delete delegated", "delegated");
  await send(ownerClient, "/new owned", null);
  await send(ownerClient, "private room text", "owned");
  await send(otherClient, "/join owned", null, "error");
  assert.equal((await request("history?view=owned", otherCookie)).status, 403);
  await send(readerClient, "/man grant", null);
  assert.ok(readerClient.frames.at(-1).text.includes("scope permission"));
  await send(readerClient, "/man revoke", null);
  assert.ok(readerClient.frames.at(-1).text.includes("direct permission"));
  await send(readerClient, "/man unknown", null, "error");
  await send(ownerClient, "/grant reader owned r:message.read", "owned");
  await send(ownerClient, "/grant reader owned /history", "owned");
  await until(
    () => readerClient.snapshot.rooms.some((r) => r.name === "owned"),
    "read-only live grant",
  );
  assert.ok(
    !readerClient.snapshot.rooms[0].permissions.includes("w:message.create"),
  );
  assert.deepEqual(readerClient.snapshot.rooms[0].members, []);
  await send(readerClient, "denied write", "owned", "error");
  await send(readerClient, "/history", "owned");
  await send(ownerClient, "/grant reader su", null, "error");
  child.stdin.write("/grant reader su\n");
  await until(
    () => readerClient.snapshot.groups.includes("su"),
    "live su membership",
  );
  await send(readerClient, "/grant other su", null);
  await until(
    () => otherClient.snapshot.groups.includes("su"),
    "su delegates su",
  );
  const protectedMessage = readerClient.snapshot.rooms.find(
    (r) => r.name === "owned",
  ).messages[0];
  await send(readerClient, `/retract ${protectedMessage.id}`, "owned");
  await until(
    () =>
      ownerClient.snapshot.rooms.find((r) => r.name === "owned").messages
        .length === 0,
    "su retracts another author",
  );
  await send(ownerClient, "/disable reader", null, "error");
  await send(otherClient, "/revoke reader su", null);
  await until(
    () => !readerClient.snapshot.groups.includes("su"),
    "live su revoke",
  );
  await send(readerClient, "write revoked", "owned", "error");
  await send(ownerClient, "/revoke reader owned r:message.read", "owned");
  await until(
    () => !readerClient.snapshot.rooms.some((r) => r.name === "owned"),
    "live read revoke",
  );
  assert.equal((await request("history?view=owned", readerCookie)).status, 403);
  await stop();
  await start(join(temporary, "production"), true, undefined, "/commonroom/");
  const bare = await fetch(base, {
    headers: { "X-Forwarded-Proto": "https" },
    redirect: "manual",
  });
  assert.equal(bare.status, 404);
  assert.equal(
    (
      await fetch(new URL("/api/health", base), {
        headers: { "X-Forwarded-Proto": "https" },
      })
    ).status,
    404,
  );
  assert.equal(
    (await request("health", null, undefined, publicOrigin, null)).status,
    400,
  );
  assert.equal(
    (await request("health", null, undefined, publicOrigin, "http")).status,
    400,
  );
  assert.equal((await request("health")).status, 200);
  const health = await request("health");
  assert.match(health.headers.get("strict-transport-security"), /max-age=/);
  assert.match(
    health.headers.get("content-security-policy"),
    /wss:\/\/chat\.example\.com/,
  );
  for (const path of ["/", "/index.html", "/assets/app.js"]) {
    const response = await fetch(`${base}${path}`, {
      headers: { "X-Forwarded-Proto": "https" },
    });
    assert.equal(response.status, 404, "server must not serve UI assets");
  }
  child.stdin.write("/user alice alice-long-password admin\n");
  await until(
    () => logs.includes("Account alice created."),
    "production account provisioning",
  );
  const secureLogin = await request("login", null, {
    username: "alice",
    password: "alice-long-password",
  });
  assert.equal(secureLogin.status, 200);
  assert.match(secureLogin.headers.get("set-cookie"), /; Secure/);
  assert.match(secureLogin.headers.get("set-cookie"), /Path=\/commonroom\//);
  const secureCookie = secureLogin.headers.get("set-cookie").split(";")[0];
  const secureClient = await connect(secureCookie);
  await send(secureClient, "/new production", null);
  await send(secureClient, "production websocket works", "production");
  assert.equal(
    (await request("logout", secureCookie, {}, "https://evil.example")).status,
    403,
  );
  const rejectedSocket = new WebSocket(`${base.replace("http:", "ws:")}/ws`, {
    headers: {
      Cookie: secureCookie,
      Origin: "https://evil.example",
      "X-Forwarded-Proto": "https",
    },
  });
  let rejected;
  rejectedSocket.on("error", (error) => {
    rejected = error;
  });
  await until(() => rejected, "untrusted WebSocket origin rejection");
  assert.match(rejected.message, /403/);
  assert.equal((await request("logout", secureCookie, {})).status, 200);
  await until(
    () => secureClient.ws.readyState === WebSocket.CLOSED,
    "logout disconnects socket",
  );
  await stop();
  await start(join(temporary, "untrusted-proxy"), true, "10.0.0.2");
  const spoofed = await fetch(`${base}/api/health`, {
    headers: {
      "X-Forwarded-Proto": "https",
      "X-Forwarded-For": "10.0.0.2",
      Origin: publicOrigin,
    },
  });
  assert.equal(
    spoofed.status,
    403,
    "forwarded headers cannot impersonate a trusted socket peer",
  );
  console.log(
    "PASS: online presence across tabs, room/private message retraction, agent roles and credential redaction, agent provider and personality settings, account deletion and username reuse, login, cross-device unread syncing, partial reads beyond snapshot tails, independent private retention, second-device directories, roles, privacy, revocation, folder migration, API-only routing, production HTTPS, secure cookies, and proxy IP trust.",
  );
} finally {
  try {
    await stop();
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
