import { test } from "node:test";
import assert from "node:assert/strict";
import {
  conversationActivity,
  conversationList,
  conversationPreview,
} from "../src/features/conversation/conversationList.ts";

const unread = (through) => ({
  count: 0,
  first: null,
  through,
  oldest: 0,
  revision: 1,
});

const views = (...args) => conversationList(...args).map(({ view }) => view);

test("unread rooms and private chats lead by newest activity", () => {
  assert.deepEqual(
    views([{ name: "lobby" }, { name: "games" }], ["alice", "bob"], {
      lobby: { ...unread(4), count: 2, first: 3 },
      games: unread(9),
      "@direct:alice": unread(7),
      "@direct:bob": { ...unread(6), count: 1, first: 6 },
    }),
    ["@direct:bob", "lobby", "@direct:alice", "games"],
  );
});

test("read and empty chats share one alphabetical order by displayed name", () => {
  assert.deepEqual(
    views(
      [{ name: "zebra" }, { name: "beta" }],
      ["Carol", "alice"],
      { zebra: unread(0), "@direct:alice": unread(3) },
      { alice: "Zoe" },
    ),
    ["beta", "@direct:Carol", "zebra", "@direct:alice"],
  );
});

test("reading a room returns it to its alphabetical place among private chats", () => {
  const rooms = [{ name: "b" }, { name: "a" }];
  const peers = ["alice"];
  const read = { a: unread(3), b: unread(6), "@direct:alice": unread(5) };
  const alphabetical = ["a", "@direct:alice", "b"];
  assert.deepEqual(views(rooms, peers, read), alphabetical);
  assert.deepEqual(
    views(rooms, peers, { ...read, b: { ...read.b, count: 1, first: 6 } }),
    ["b", "a", "@direct:alice"],
  );
  assert.deepEqual(views(rooms, peers, read), alphabetical);
});

test("inspected private rooms sort by their displayed pair", () => {
  assert.deepEqual(
    conversationList([{ name: "@private:ann:bo" }, { name: "lobby" }], [], {}),
    [
      { view: "@private:ann:bo", kind: "room", name: "ann:bo" },
      { view: "lobby", kind: "room", name: "lobby" },
    ],
  );
});

test("chat activity keeps the latest timestamp separate from the newest unread preview", () => {
  const roomUnread = { ...unread(8), oldest: 4, first: 5, count: 2 };
  const incoming = { sequence: 6, from: "bob" };
  const sent = { sequence: 8, from: "alice" };
  const direct = {
    sequence: 7,
    from: "carol",
    to: "alice",
    private_id: "current",
  };
  const activity = conversationActivity({
    username: "alice",
    rooms: [
      {
        name: "lobby",
        messages: [
          { sequence: 3, from: "bob" },
          sent,
          incoming,
          { sequence: 5, from: "carol" },
        ],
      },
    ],
    direct: [direct, { ...direct, sequence: 9, private_id: "revoked" }],
    private_access: { "@direct:carol": { id: "current" } },
    unread: {
      lobby: roomUnread,
      "@direct:carol": { ...unread(9), first: 7, count: 1 },
    },
  });
  assert.deepEqual(activity.get("lobby"), { latest: sent, unread: incoming });
  assert.deepEqual(activity.get("@direct:carol"), {
    latest: direct,
    unread: direct,
  });
});

test("chat previews show plain text and useful image labels", () => {
  assert.equal(
    conversationPreview({
      markdown: true,
      text: "**Hello**, [world](https://example.com)\n\n![Sunset](https://example.com/photo.png)",
    }),
    "Hello, world Sunset",
  );
  assert.equal(conversationPreview({ text: " plain\n text  " }), "plain text");
  assert.equal(
    conversationPreview({ text: "🙂".repeat(161) }),
    `${"🙂".repeat(159)}…`,
  );
});
