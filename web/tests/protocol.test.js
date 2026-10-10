import assert from "node:assert/strict";
import test from "node:test";
import { isSnapshot, isHistory, parseFrame } from "../src/api/protocol.ts";

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
  rooms: [
    {
      id: "room-id",
      owner: "alice",
      permissions: [],
      commands: [],
      name: "room",
      members: ["alice", "bob"],
      messages: [message],
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
  assert.ok(isHistory({ view: "room", messages: [message], revision: 1 }));
  for (const frame of [
    snapshot,
    { kind: "read", unread: snapshot.unread },
    { kind: "notice", id: 1, text: "" },
    { kind: "error", id: 2, text: "Denied" },
  ])
    assert.deepEqual(parseFrame(JSON.stringify(frame)), frame);
});
test("protocol rejects malformed nested data before it reaches UI state", () => {
  for (const invalid of [
    null,
    {},
    { ...snapshot, account_access: null },
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
