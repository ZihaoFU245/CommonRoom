import { test } from "node:test";
import assert from "node:assert/strict";
import {
  ingest,
  timeline,
  suggestions,
  privatePeers,
  privateMessages,
  redactCommand,
  commandText,
  clearedOrder,
} from "../src/features/conversation/console.ts";

test("system subcommands complete only when the server advertises su access", () => {
  const commands = [
    {
      name: "/system",
      usage: "/system configs | metrics",
      description: "System operations",
    },
    { name: "/sudo", usage: "/sudo /command", description: "Elevate" },
  ];
  assert.deepEqual(
    suggestions("/system ", commands, [], []).map((hint) => hint.value),
    ["/system configs ", "/system metrics "],
  );
  assert.equal(
    suggestions("/system m", commands, [], [])[0].value,
    "/system metrics ",
  );
  assert.equal(
    suggestions("/sudo /system m", commands, [], [])[0].value,
    "/sudo /system metrics ",
  );
  assert.deepEqual(suggestions("/system ", [], [], []), []);
});

test("sudo completion uses elevated commands without exposing them directly", () => {
  const system = {
    name: "/system",
    usage: "/system configs | metrics",
    description: "System operations",
  };
  const sudo = {
    name: "/sudo",
    usage: "/sudo /command",
    description: "Elevate",
    subcommands: [system],
  };
  const commands = [sudo];
  assert.equal(
    suggestions("/sudo ", commands, [], [])[0].value,
    "/sudo /system ",
  );
  assert.deepEqual(suggestions("/sys", commands, [], []), []);
  assert.deepEqual(suggestions("/system ", commands, [], []), []);
  assert.equal(
    suggestions("/sudo /sys", commands, [], [])[0].value,
    "/sudo /system ",
  );
  assert.deepEqual(
    suggestions("/sudo /system ", commands, [], []).map((hint) => hint.value),
    ["/sudo /system configs ", "/sudo /system metrics "],
  );
  for (const subcommand of ["configs", "metrics"])
    assert.equal(
      suggestions(`/sudo /system ${subcommand[0]}`, commands, [], [])[0].value,
      `/sudo /system ${subcommand} `,
    );
  assert.deepEqual(suggestions("/sudo /sudo ", commands, [], []), []);
  assert.deepEqual(
    suggestions("/sudo /sys", [{ ...sudo, subcommands: undefined }], [], []),
    [],
  );
  assert.equal(
    suggestions("/sudo /sys", [...commands, system], [], []).length,
    1,
  );
});

test("prototype member room names preserve transcript and local clear behavior", () => {
  const message = { id: "one", sequence: 1, time: 1 };
  const ledger = { sequence: 2, messages: new Map([["one", 1]]) };
  for (const view of [
    "toString",
    "constructor",
    "valueOf",
    "hasOwnProperty",
    "propertyIsEnumerable",
    "toLocaleString",
    "__proto__",
  ]) {
    const output = [{ kind: "command", key: "local", room: view, order: 2 }];
    assert.equal(
      timeline([message], output, ledger, clearedOrder({}, view), view).length,
      2,
    );
    assert.equal(
      timeline(
        [message],
        output,
        ledger,
        clearedOrder({ [view]: 2 }, view),
        view,
      ).length,
      0,
    );
  }
});

test("account command passwords are masked", () => {
  assert.equal(
    redactCommand("/user passwd old secret"),
    "/user passwd •••• ••••",
  );
  assert.equal(
    redactCommand("/user new bob secret user"),
    "/user new bob •••• user",
  );
});

test("sudo passwords stay masked and wrapped commands retain completion and UI routing", () => {
  assert.equal(
    redactCommand("/sudo /user passwd old secret"),
    "/sudo /user passwd •••• ••••",
  );
  assert.equal(
    redactCommand("/sudo /user new bob secret user"),
    "/sudo /user new bob •••• user",
  );
  assert.equal(
    redactCommand("/sudo /user reset bob password secret"),
    "/sudo /user reset bob password ••••",
  );
  assert.equal(
    redactCommand("/sudo /sudo /user reset bob password secret"),
    "/sudo /sudo /user reset bob password ••••",
  );
  assert.equal(commandText(" /sudo /room new team "), "/room new team");
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
    { name: "/room", usage: "/room join #room", description: "Switch" },
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
    suggestions("/room join #l", commands, ["bob"], ["lobby"])[0].value,
    "/room join #lobby ",
  );
  assert.deepEqual(
    suggestions("/tell bob hello", commands, ["bob"], ["lobby"]),
    [],
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
    suggestions("/grant bob user #s", commands, ["bob"], ["support"])[0].value,
    "/grant bob user #support ",
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
  for (const draft of ["/rep", "/grant bob #support /rep"]) {
    const hint = suggestions(draft, commands, ["bob"], ["support"])[0];
    assert.ok(hint.description.includes("r:message.read + w:message.create"));
  }
  assert.equal(
    suggestions("/grant bob #support /rep", commands, ["bob"], ["support"])[0]
      .value,
    "/grant bob #support /reply ",
  );
  assert.equal(
    suggestions("/grant bob @server", commands, ["bob"], ["support"]).some(
      (hint) => hint.value === "/grant bob @server ",
    ),
    false,
  );
});

test("metadata reset arguments remain visible while password reset values are masked", () => {
  for (const command of [
    "/user reset bob @all #a",
    "/user reset bob media.max_age @global",
    "/sudo /user reset @all @all #a",
  ])
    assert.equal(redactCommand(command), command);
  assert.equal(
    redactCommand("/user reset bob password secret"),
    "/user reset bob password ••••",
  );
});
