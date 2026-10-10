import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ingest,
  timeline,
  suggestions,
  privatePeers,
  privateMessages,
  redactCommand,
  helpSections,
  commandText,
} from "../src/features/conversation/console.ts";

test("passwords are masked and help groups retain their commands", () => {
  assert.equal(redactCommand("/passwd old secret"), "/passwd •••• ••••");
  assert.equal(redactCommand("/user bob secret user"), "/user bob •••• user");
  assert.equal(
    redactCommand("/agent helper sk-9f6bce6e3fcd4e2889d1a8e858adfef7"),
    "/agent helper ••••",
  );
  assert.equal(redactCommand("/agent helper"), "/agent helper");
  assert.equal(
    redactCommand("/agent-key sk-9f6bce6e3fcd4e2889d1a8e858adfef7"),
    "/agent-key ••••",
  );
  assert.equal(
    redactCommand("/agent-key sk-9f6bce6e3fcd4e2889d1a8e858adfef7 helper"),
    "/agent-key •••• helper",
  );
  assert.equal(
    redactCommand("/agent-reply auto helper"),
    "/agent-reply auto helper",
  );
  assert.equal(redactCommand("/agent-name helper"), "/agent-name helper");
  assert.deepEqual(
    helpSections("[General]\n/help — Help\n\n[Rooms]\n/kick user — Kick"),
    [
      { title: "General", commands: [{ usage: "/help", description: "Help" }] },
      {
        title: "Rooms",
        commands: [{ usage: "/kick user", description: "Kick" }],
      },
    ],
  );
});

test("sudo passwords stay masked and wrapped commands retain completion and UI routing", () => {
  assert.equal(
    redactCommand("/sudo /passwd old secret"),
    "/sudo /passwd •••• ••••",
  );
  assert.equal(
    redactCommand("/sudo /user bob secret user"),
    "/sudo /user bob •••• user",
  );
  assert.equal(
    redactCommand("/sudo /reset bob secret"),
    "/sudo /reset bob ••••",
  );
  assert.equal(
    redactCommand("/sudo /sudo /reset bob secret"),
    "/sudo /sudo /reset bob ••••",
  );
  assert.equal(commandText(" /sudo /new team "), "/new team");
  assert.equal(commandText("/retract message"), "/retract message");
  const commands = [
    {
      name: "/sudo",
      usage: "/sudo /command [arguments]",
      description: "Elevate",
    },
    { name: "/tell", usage: "/tell user message", description: "Private" },
  ];
  assert.equal(
    suggestions("/sudo /te", commands, ["bob"], ["team"])[0].value,
    "/sudo /tell ",
  );
  assert.equal(
    suggestions("/sudo /tell b", commands, ["bob"], ["team"])[0].value,
    "/sudo /tell bob ",
  );
  assert.deepEqual(suggestions("/sudo /sudo ", commands, [], []), []);
});

test("local command output stays in its origin view", () => {
  const output = [
    { key: "room", room: "lol", order: 1 },
    { key: "command", room: "@command", order: 2 },
    { key: "private", room: "@direct:bob", order: 3 },
  ];
  const ledger = { messages: new Map() };
  assert.deepEqual(
    timeline([], output, ledger, 0, "lol").map((entry) => entry.key),
    ["room"],
  );
  assert.deepEqual(
    timeline([], output, ledger, 0, "@command").map((entry) => entry.key),
    ["command"],
  );
  assert.equal(timeline([], output, ledger, 3, "@direct:bob").length, 0);
  assert.equal(timeline([], [], ledger, 0, "lol").length, 0);
});
test("private conversations contain only messages between the selected people", () => {
  const direct = [
    { from: "alice", to: "bob", text: "outgoing" },
    { from: "bob", to: "alice", text: "incoming" },
    { from: "carol", to: "alice", text: "other" },
  ];
  assert.deepEqual(
    privateMessages(direct, "alice", "bob").map((message) => message.text),
    ["outgoing", "incoming"],
  );
  assert.deepEqual(
    privatePeers({
      username: "alice",
      users: ["alice", "bob", "dave"],
      direct,
    }),
    ["bob", "carol"],
  );
  assert.deepEqual(
    privatePeers({ username: "alice", users: ["alice", "bob"], direct: [] }),
    [],
  );
  assert.deepEqual(privatePeers({ private_peers: ["bob"], direct: [] }), [
    "bob",
  ]);
  assert.deepEqual(privateMessages(direct, "alice", null), []);
});

test("snapshots preserve output ordering, clear is local, refresh only restores chat", () => {
  const first = { id: "first", time: 1, text: "retained" };
  const initial = { rooms: [{ messages: [first] }], direct: [] };
  const ledger = ingest(initial);
  const output = [
    {
      kind: "command",
      key: "local",
      order: ++ledger.sequence,
      result: "admin",
    },
  ];
  const next = { id: "next", time: 1, text: "live" };
  ingest({ rooms: [{ messages: [first, next] }], direct: [] }, ledger);
  assert.deepEqual(
    timeline([first, next], output, ledger).map((e) => e.key),
    ["first", "local", "next"],
  );
  assert.equal(
    timeline([first, next], output, ledger, ledger.sequence).length,
    0,
  );
  const refreshed = ingest({
    rooms: [{ messages: [first, next] }],
    direct: [],
  });
  assert.deepEqual(
    timeline([first, next], [], refreshed).map((e) => e.key),
    ["first", "next"],
  );
});
test("hints appear only for slash commands and complete usernames or rooms", () => {
  const commands = [
    { name: "/tell", usage: "/tell user message", description: "Private" },
    { name: "/join", usage: "/join room", description: "Switch" },
  ];
  assert.deepEqual(suggestions("hello", commands, ["bob"], ["lobby"]), []);
  assert.equal(
    suggestions("/te", commands, ["bob"], ["lobby"])[0].value,
    "/tell ",
  );
  assert.equal(
    suggestions("/tell b", commands, ["bob"], ["lobby"])[0].value,
    "/tell bob ",
  );
  assert.equal(
    suggestions("/join l", commands, ["bob"], ["lobby"])[0].value,
    "/join lobby ",
  );
  assert.equal(
    suggestions("/tell bob hello", commands, ["bob"], ["lobby"])[0].value,
    null,
  );
});

test("delete-user hints complete usernames rather than rooms", () => {
  const commands = [
    {
      name: "/deleteuser",
      usage: "/deleteuser user",
      description: "Delete account",
    },
  ];
  assert.equal(
    suggestions("/deleteu", commands, ["bob"], ["board"])[0].value,
    "/deleteuser ",
  );
  assert.deepEqual(
    suggestions("/deleteuser b", commands, ["bob"], ["board"]).map(
      (h) => h.value,
    ),
    ["/deleteuser bob "],
  );
});

test("private inspectors' messages stay in their original participant pair", () => {
  const messages = [
    { from: "moderator", to: "alice", private_id: "pair", text: "review" },
    { from: "moderator", to: "alice", private_id: "other", text: "elsewhere" },
  ];
  assert.deepEqual(
    privateMessages(messages, "alice", "bob", "pair").map((m) => m.text),
    ["review"],
  );
});

test("manual topics and both grant forms have contextual completion", () => {
  const commands = [
    { name: "/grant", usage: "/grant user group [room]", description: "Grant" },
    {
      name: "/revoke",
      usage: "/revoke user scope permission",
      description: "Revoke",
    },
    { name: "/man", usage: "/man [topic]", description: "Manual" },
  ];
  assert.equal(suggestions("/man g", commands, [], [])[0].value, "/man grant ");
  assert.equal(
    suggestions("/revoke bob su", commands, ["bob"], ["support"])[0].value,
    "/revoke bob su ",
  );
  assert.equal(
    suggestions("/grant bob @g", commands, ["bob"], ["support"])[0].value,
    "/grant bob @global ",
  );
  assert.equal(
    suggestions("/grant bob user s", commands, ["bob"], ["support"])[0].value,
    "/grant bob user support ",
  );
});

test("command and grant hints disclose all action requirements without granting access", () => {
  const commands = [
    {
      name: "/grant",
      usage: "/grant user scope permission",
      description: "Grant",
    },
    {
      name: "/reply",
      usage: "/reply id text",
      description: "Reply",
      requirements: "r:message.read + w:message.create on the conversation",
    },
  ];
  for (const draft of [
    "/rep",
    "/reply id text",
    "/grant bob support /rep",
    "/grant bob support /reply ",
  ]) {
    const hint = suggestions(draft, commands, ["bob"], ["support"])[0];
    assert.ok(hint.description.includes("r:message.read + w:message.create"));
  }
  assert.equal(
    suggestions("/grant bob support /rep", commands, ["bob"], ["support"])[0]
      .value,
    "/grant bob support /reply ",
  );
  assert.equal(
    suggestions("/grant bob @server", commands, ["bob"], ["support"]).some(
      (hint) => hint.value === "/grant bob @server ",
    ),
    false,
  );
});

test("agent hints complete reply modes first and agent names second", () => {
  const commands = [
    {
      name: "/agent-reply",
      usage: "/agent-reply [auto|mention] [agent-name]",
      description: "Reply mode",
    },
    {
      name: "/agent-remove",
      usage: "/agent-remove [agent-name]",
      description: "Remove agent",
    },
  ];
  assert.deepEqual(
    suggestions("/agent-reply ", commands, [], [], ["helper", "writer"]).map(
      (h) => h.value,
    ),
    ["/agent-reply auto ", "/agent-reply mention "],
  );
  assert.deepEqual(
    suggestions(
      "/agent-reply mention ",
      commands,
      [],
      [],
      ["helper", "writer"],
    ).map((h) => h.value),
    ["/agent-reply mention helper ", "/agent-reply mention writer "],
  );
  assert.deepEqual(
    suggestions("/agent-remove w", commands, [], [], ["helper", "writer"]).map(
      (h) => h.value,
    ),
    ["/agent-remove writer "],
  );
  assert.equal(
    suggestions("/agent-remove ", commands, [], [], [])[0].value,
    null,
  );
});
