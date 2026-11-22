import { test } from "node:test";
import assert from "node:assert/strict";
import { conversationList } from "../src/features/conversation/conversationList.ts";

const unread = (through) => ({
  count: 0,
  first: 0,
  through,
  oldest: 0,
  revision: 1,
});

const views = (...args) => conversationList(...args).map(({ view }) => view);

test("conversations order by latest message across rooms and private chats", () => {
  assert.deepEqual(
    views([{ name: "lobby" }, { name: "games" }], ["alice", "bob"], [], {
      lobby: unread(4),
      games: unread(9),
      "@direct:alice": unread(7),
      "@direct:bob": unread(2),
    }),
    ["games", "@direct:alice", "lobby", "@direct:bob"],
  );
});

test("conversations without messages follow by name, mixing rooms and peers", () => {
  assert.deepEqual(
    views([{ name: "zebra" }, { name: "beta" }], ["Carol", "alice"], [], {
      zebra: unread(0),
      "@direct:alice": unread(3),
    }),
    ["@direct:alice", "beta", "@direct:Carol", "zebra"],
  );
});

test("unsent private drafts stay on top, newest draft first", () => {
  assert.deepEqual(
    views(
      [{ name: "lobby" }],
      ["alice", "bob", "carol"],
      ["carol", "alice", "bob"],
      { lobby: unread(5), "@direct:bob": unread(6) },
    ),
    ["@direct:alice", "@direct:carol", "@direct:bob", "lobby"],
  );
});

test("inspected private rooms sort by their displayed pair", () => {
  assert.deepEqual(
    conversationList(
      [{ name: "@private:ann:bo" }, { name: "lobby" }],
      [],
      [],
      {},
    ),
    [
      { view: "@private:ann:bo", kind: "room", name: "ann:bo", latest: 0 },
      { view: "lobby", kind: "room", name: "lobby", latest: 0 },
    ],
  );
});
