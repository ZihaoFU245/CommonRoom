import assert from "node:assert/strict";
import test from "node:test";
import {
  isSnapshot,
  isHistory,
  isMessage,
  parseFrame,
} from "../src/api/protocol.ts";

test("message timestamps stay within the JavaScript Date range", () => {
  for (const time of [0, 8_640_000_000_000])
    assert.equal(isMessage({ ...message, time }), true);
  for (const time of [8_640_000_000_001, 1e15, NaN, Infinity, -1])
    assert.equal(isMessage({ ...message, time }), false);
});

const message = {
  id: "one",
  from: "alice",
  to: null,
  text: "你好 مرحبا 👋",
  time: 10,
  sequence: 1,
  reactions: { 赞: ["bob"] },
  reply: { id: "old", from: "bob", text: "hello" },
  mentions: ["bob"],
};
test("reply quotes may carry the original's sequence", () => {
  assert.equal(
    isMessage({ ...message, reply: { ...message.reply, sequence: 7 } }),
    true,
  );
  assert.equal(isMessage(message), true);
  for (const sequence of [-1, 1.5, "7"])
    assert.equal(
      isMessage({ ...message, reply: { ...message.reply, sequence } }),
      false,
    );
});
const snapshot = {
  kind: "snapshot",
  username: "alice",
  groups: ["user"],
  permissions: [],
  account_access: { id: "account-id", permissions: [], commands: [] },
  private_access: {},
  private_permissions: [],
  private_commands: [],
  policy_revision: 0,
  admin: true,
  users: ["alice", "bob"],
  online: ["alice"],
  colors: { alice: "red", "用户🙂": "teal" },
  rooms: [
    {
      id: "room-id",
      owner: "alice",
      permissions: [],
      commands: [],
      name: "room",
      members: ["alice", "bob"],
      messages: [message],
      colors: { bob: "blue" },
    },
  ],
  direct: [],
  private_peers: [],
  commands: [
    {
      name: "/help",
      usage: "/help",
      description: "Help",
      section: "General",
      admin: false,
      console: false,
    },
  ],
  available_rooms: ["room"],
  unread: { room: { count: 1, first: 1, through: 1, oldest: 1, revision: 1 } },
};
test("protocol accepts complete Unicode snapshots, history, and every server frame", () => {
  assert.ok(isSnapshot(snapshot));
  assert.ok(
    isSnapshot({
      ...snapshot,
      commands: [
        {
          ...snapshot.commands[0],
          requirements: "r:message.read + w:message.create",
        },
      ],
    }),
  );
  assert.ok(
    isHistory({
      view: "room",
      messages: [message],
      revision: 1,
      has_before: false,
      has_after: false,
    }),
  );
  for (const frame of [
    snapshot,
    { kind: "read", unread: snapshot.unread },
    { kind: "notice", id: 1, text: "" },
    { kind: "error", id: 2, text: "Denied" },
  ])
    assert.deepEqual(parseFrame(JSON.stringify(frame)), frame);
});
test("sudo completion metadata validates wrapped commands with bounded nesting", () => {
  const system = { ...snapshot.commands[0], name: "/system" };
  const sudo = {
    ...snapshot.commands[0],
    name: "/sudo",
    subcommands: [system],
  };
  const value = { ...snapshot, commands: [sudo] };
  assert.deepEqual(parseFrame(JSON.stringify(value)), value);
  for (const subcommands of [null, {}, [{}], [{ ...system, usage: 1 }], [sudo]])
    assert.equal(
      parseFrame(
        JSON.stringify({ ...snapshot, commands: [{ ...sudo, subcommands }] }),
      ),
      null,
    );
});

test("protocol rejects malformed nested data before it reaches UI state", () => {
  for (const invalid of [
    null,
    {},
    { ...snapshot, account_access: null },
    { ...snapshot, colors: ["red"] },
    { ...snapshot, colors: { alice: 1 } },
    { ...snapshot, rooms: [{ ...snapshot.rooms[0], colors: ["red"] }] },
    { ...snapshot, rooms: [{ ...snapshot.rooms[0], colors: { bob: 1 } }] },
    {
      ...snapshot,
      account_access: { id: "id", permissions: [123], commands: [] },
    },
    {
      ...snapshot,
      account_access: { id: "id", permissions: [], commands: [{}] },
    },
    { ...snapshot, online: [123] },
    { ...snapshot, groups: [123] },
    { ...snapshot, policy_revision: -1 },
    { ...snapshot, private_permissions: null },
    { ...snapshot, private_commands: [{}] },
    {
      ...snapshot,
      private_access: {
        "@direct:bob": { id: "pair", permissions: [], commands: [{}] },
      },
    },
    { ...snapshot, rooms: [{ ...snapshot.rooms[0], permissions: [123] }] },
    { ...snapshot, rooms: [{ ...snapshot.rooms[0], commands: [{}] }] },
    { ...snapshot, rooms: [{ ...snapshot.rooms[0], members: "alice" }] },
    { ...snapshot, direct: [{ ...message, reactions: { "👍": "bob" } }] },
    { ...snapshot, direct: [{ ...message, reply: { id: "old" } }] },
    { ...snapshot, unread: { room: { ...snapshot.unread.room, count: -1 } } },
    { ...snapshot, commands: [{ ...snapshot.commands[0], admin: "true" }] },
    {
      ...snapshot,
      commands: [{ ...snapshot.commands[0], requirements: ["w:room.create"] }],
    },
    { ...snapshot, direct: [{ ...message, sequence: 1.5 }] },
  ]) {
    assert.equal(isSnapshot(invalid), false);
    assert.equal(parseFrame(JSON.stringify(invalid)), null);
  }
  for (const raw of [
    "{",
    JSON.stringify({ kind: "notice", id: "1", text: "ok" }),
    JSON.stringify({ kind: "read", unread: [] }),
  ])
    assert.equal(parseFrame(raw), null);
  assert.equal(
    isHistory({
      view: "room",
      revision: 1,
      messages: [{ ...message, text: null }],
    }),
    false,
  );
});

test("history and incremental frames reject unsafe sequences and oversized pages", () => {
  const history = {
    view: "room",
    revision: 1,
    messages: [message],
    has_before: true,
    has_after: false,
  };
  assert.equal(isHistory(history), true);
  assert.equal(
    isHistory({ ...history, messages: Array(100).fill(message) }),
    true,
  );
  for (const invalid of [
    { ...history, messages: Array(101).fill(message) },
    { ...history, has_before: 1 },
    { ...history, has_after: undefined },
    { ...history, revision: Number.MAX_SAFE_INTEGER + 1 },
    {
      ...history,
      messages: [{ ...message, sequence: Number.MAX_SAFE_INTEGER + 1 }],
    },
  ])
    assert.equal(isHistory(invalid), false);
  const frame = {
    kind: "conversation",
    view: "room",
    id: "stable",
    messages: [message],
    unread: snapshot.unread.room,
  };
  assert.deepEqual(parseFrame(JSON.stringify(frame)), frame);
  assert.equal(
    parseFrame(JSON.stringify({ ...frame, messages: Array(51).fill(message) })),
    null,
  );
  assert.equal(
    parseFrame(
      JSON.stringify({ ...frame, unread: { ...frame.unread, through: -1 } }),
    ),
    null,
  );
});

test("edited markers accept legacy messages and reject malformed values", () => {
  const history = {
    view: "room",
    revision: 1,
    messages: [message],
    has_before: false,
    has_after: false,
  };
  for (const edited of [undefined, false, true]) {
    assert.equal(
      isHistory({ ...history, messages: [{ ...message, edited }] }),
      true,
    );
  }
  for (const edited of [null, "true", 1, {}]) {
    assert.equal(
      isHistory({ ...history, messages: [{ ...message, edited }] }),
      false,
    );
  }
});

test("presence frames validate the online directory without carrying history", () => {
  assert.deepEqual(
    parseFrame(JSON.stringify({ kind: "presence", online: ["alice", "界"] })),
    { kind: "presence", online: ["alice", "界"] },
  );
  for (const online of [null, {}, "alice", ["alice", 42]])
    assert.equal(
      parseFrame(JSON.stringify({ kind: "presence", online })),
      null,
    );
});

test("avatar directory accepts optional string maps and rejects malformed profiles", () => {
  assert.equal(
    isSnapshot({
      ...snapshot,
      avatars: { alice: "https://example.com/a.png" },
    }),
    true,
  );
  for (const avatars of [null, [], "url", { alice: 42 }, { alice: null }])
    assert.equal(isSnapshot({ ...snapshot, avatars }), false);
});

test("avatar frames accept explicit removals and reject malformed profiles", () => {
  for (const url of [
    "https://example.com/a.png",
    "/api/avatars/id/file",
    null,
  ]) {
    const value = { kind: "avatar", username: "alice", url };
    assert.deepEqual(parseFrame(JSON.stringify(value)), value);
  }
  for (const value of [
    { kind: "avatar", username: "alice" },
    { kind: "avatar", username: "", url: null },
    { kind: "avatar", username: 1, url: null },
    { kind: "avatar", username: "alice", url: {} },
  ]) {
    assert.equal(parseFrame(JSON.stringify(value)), null);
  }
});

test("history and live color projections validate nullable values and versions", () => {
  const page = {
    view: "team",
    messages: [],
    revision: 1,
    has_before: false,
    has_after: false,
  };
  const update = {
    kind: "conversation",
    view: "team",
    id: "stable",
    messages: [],
    unread: { count: 0, first: null, oldest: 0, through: 0, revision: 1 },
  };
  for (const name_colors of [
    { version: "instance:1", values: { alice: "red", bob: null } },
    { version: "instance:1", values: {} },
  ]) {
    assert.ok(isHistory({ ...page, name_colors }));
    assert.deepEqual(
      parseFrame(JSON.stringify({ ...update, name_colors })).name_colors,
      name_colors,
    );
  }
  for (const name_colors of [
    null,
    {},
    { version: 1, values: {} },
    { version: "one", values: [] },
    { version: "one", values: { alice: 1 } },
  ]) {
    assert.equal(isHistory({ ...page, name_colors }), false);
    assert.equal(parseFrame(JSON.stringify({ ...update, name_colors })), null);
  }
});
