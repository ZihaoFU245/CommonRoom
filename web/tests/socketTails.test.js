import assert from "node:assert/strict";
import test from "node:test";
import { parseFrame } from "../src/api/protocol.ts";
import { SocketTails } from "../src/features/conversation/socketTails.ts";

test("deltas replace edits, drop deleted rows, and reject a missing base", () => {
  const message = (sequence, text) => ({
    sequence,
    id: String(sequence),
    from: "alice",
    to: null,
    text,
    time: 1,
    reactions: {},
    reply: null,
    mentions: [],
  });
  const unread = { revision: 5, count: 0, first: null, oldest: 1, through: 3 };
  const first = message(1, "first");
  const second = message(2, "second");
  const edited = { ...first, reactions: { "👍": ["alice"] } };
  const third = message(3, "third");
  const tails = new SocketTails({
    rooms: [{ id: "id", name: "room", messages: [first, second] }],
    unread: { room: { ...unread, revision: 3 } },
    private_access: {},
    direct: [],
  });
  const frame = {
    kind: "conversation_delta",
    view: "room",
    id: "id",
    base_revision: 3,
    sequences: [1, 3],
    upsert: [edited, third],
    unread,
  };
  const parse = (value) => parseFrame(JSON.stringify(value));
  assert.equal(parse({ ...frame, sequences: [3, 1] }), null);
  assert.equal(tails.apply(parse({ ...frame, upsert: [edited] })), null);
  assert.equal(tails.apply(parse({ ...frame, base_revision: 4 })), null);
  assert.deepEqual(tails.apply(parse(frame)).messages, [edited, third]);
  assert.equal(tails.apply(parse(frame)), null);
});
