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
  admin: true,
  users: ["alice", "bob"],
  online: ["alice"],
  rooms: [{ name: "room", members: ["alice", "bob"], messages: [message] }],
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
    { ...snapshot, online: [123] },
    { ...snapshot, rooms: [{ ...snapshot.rooms[0], members: "alice" }] },
    { ...snapshot, direct: [{ ...message, reactions: { "👍": "bob" } }] },
    { ...snapshot, direct: [{ ...message, reply: { id: "old" } }] },
    { ...snapshot, unread: { room: { ...snapshot.unread.room, count: -1 } } },
    { ...snapshot, commands: [{ ...snapshot.commands[0], admin: "true" }] },
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
