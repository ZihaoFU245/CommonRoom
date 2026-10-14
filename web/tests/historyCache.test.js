import assert from "node:assert/strict";
import test from "node:test";
import {
  applyConversation,
  boundSnapshot,
  bytes,
  mergePage,
  HistoryCache,
} from "../src/features/conversation/historyCache.ts";
const message = (sequence) => ({
  id: String(sequence),
  sequence,
  text: "x",
  from: "alice",
  to: null,
  time: 0,
  reactions: {},
  mentions: [],
  reply: null,
});
const page = (view, start, count = 100) => ({
  view,
  id: view,
  messages: Array.from({ length: count }, (_, index) => message(start + index)),
  revision: 1,
  through: 2000,
  oldest: 1,
  has_before: start > 1,
  has_after: start + count <= 2000,
});
test("sliding windows evict the opposite end without losing paging direction", () => {
  const previous = page("room", 101, 1000);
  const older = mergePage(previous, page("room", 1), "older");
  assert.equal(older.messages.length, 1000);
  assert.equal(older.messages[0].sequence, 1);
  assert.equal(older.messages.at(-1).sequence, 1000);
  assert.equal(older.has_after, true);
  const newer = mergePage(older, page("room", 1001), "newer");
  assert.equal(newer.messages[0].sequence, 101);
  assert.equal(newer.messages.at(-1).sequence, 1100);
  assert.equal(newer.has_before, true);
});
test("conversation caches also enforce a global entry budget", () => {
  const cache = new HistoryCache();
  for (let index = 0; index < 10; index++)
    cache.store(page(String(index), 1, 1000));
  assert.ok(
    [...cache.values()].reduce(
      (count, entry) => count + entry.messages.length,
      0,
    ) <= 5000,
  );
  assert.ok(cache.has("9"));
  assert.ok(!cache.has("0"));
});

test("repeated paging past 1000 and back recovers exact windows", () => {
  let window = page("room", 1901);
  for (let start = 1801; start >= 1; start -= 100)
    window = mergePage(window, page("room", start), "older");
  assert.equal(window.messages.length, 1000);
  assert.equal(window.messages[0].sequence, 1);
  assert.equal(window.messages.at(-1).sequence, 1000);
  assert.equal(window.has_before, false);
  assert.equal(window.has_after, true);
  for (let start = 1001; start <= 1901; start += 100)
    window = mergePage(window, page("room", start), "newer");
  assert.deepEqual(
    window.messages.map((m) => m.sequence),
    Array.from({ length: 1000 }, (_, i) => i + 1001),
  );
  assert.equal(window.has_before, true);
  assert.equal(window.has_after, false);
});
test("overlap replaces reactions without duplicating or mutating previous pages", () => {
  const current = page("room", 1);
  const update = page("room", 51);
  update.revision = 2;
  update.messages[0] = { ...update.messages[0], reactions: { ok: ["bob"] } };
  const merged = mergePage(current, update, "newer");
  assert.equal(merged.messages.length, 150);
  assert.deepEqual(merged.messages[50].reactions, { ok: ["bob"] });
  assert.deepEqual(current.messages[50].reactions, {});
  assert.equal(merged.revision, 2);
});
test("byte eviction retains the requested edge and permits an oversized single row", () => {
  const current = page("room", 101, 5);
  for (const m of current.messages) m.text = "🙂".repeat(300000);
  const older = mergePage(current, page("room", 1, 5), "older");
  assert.ok(bytes(older.messages) <= 4 * 1024 * 1024);
  assert.equal(older.messages[0].sequence, 1);
  assert.equal(older.has_after, true);
  const giant = page("room", 9999, 1);
  giant.messages[0].text = "x".repeat(3 * 1024 * 1024);
  const newer = mergePage(current, giant, "newer");
  assert.equal(newer.messages.length, 1);
  assert.equal(newer.messages[0].sequence, 9999);
  assert.equal(newer.has_before, true);
});
test("global cache evicts by recency, view count, and payload budget", () => {
  const cache = new HistoryCache();
  for (let i = 0; i < 32; i++) cache.store(page(String(i), 1, 1));
  cache.store(cache.get("0"));
  cache.store(page("new", 1, 1));
  assert.ok(cache.has("0"));
  assert.ok(!cache.has("1"));
  assert.equal(cache.size, 32);
  const large = page("large", 1, 10);
  for (const m of large.messages) m.text = "x".repeat(500000);
  cache.store(large);
  const next = { ...large, view: "next", id: "next" };
  cache.store(next);
  assert.ok(!cache.has("large"));
  assert.ok(cache.has("next"));
  assert.ok(
    [...cache.values()].reduce((n, p) => n + bytes(p.messages), 0) <=
      16 * 1024 * 1024,
  );
});
const snapshot = () => ({
  rooms: Array.from({ length: 24 }, (_, i) => ({
    id: String(i),
    name: String(i),
    messages: page(String(i), i * 100 + 1, 50).messages,
  })),
  direct: [],
  private_access: {},
  unread: {},
  users: ["alice"],
});
test("snapshot tails are globally bounded and prioritize the updated conversation", () => {
  const original = snapshot();
  const bounded = boundSnapshot(original, "23");
  assert.equal(
    bounded.rooms.reduce((n, r) => n + r.messages.length, 0),
    1000,
  );
  assert.equal(bounded.rooms[23].messages.length, 50);
  assert.equal(bounded.rooms.length, 24);
  assert.equal(original.rooms[22].messages.length, 50);
  const huge = snapshot();
  huge.rooms[0].messages.forEach((m) => (m.text = "x".repeat(100000)));
  const result = boundSnapshot(huge, "0");
  assert.ok(
    result.rooms.reduce((n, r) => n + bytes(r.messages), 0) <= 4 * 1024 * 1024,
  );
});
test("incremental updates preserve unaffected rooms and reject reused resource identity", () => {
  const original = { ...snapshot(), rooms: snapshot().rooms.slice(0, 2) };
  const update = {
    view: "0",
    id: "0",
    messages: [message(3000)],
    unread: { count: 1, first: 3000, through: 3000, oldest: 3000, revision: 2 },
  };
  const result = applyConversation(original, update);
  assert.equal(result.rooms[1], original.rooms[1]);
  assert.equal(result.rooms[0].messages[0].sequence, 3000);
  assert.equal(result.unread["0"], update.unread);
  assert.equal(
    applyConversation(original, { ...update, id: "replacement" }),
    null,
  );
  assert.equal(
    applyConversation(original, { ...update, view: "missing" }),
    null,
  );
});
test("private updates use stable pair IDs and preserve other conversations", () => {
  const a = { ...message(1), private_id: "a" };
  const b = { ...message(2), private_id: "b" };
  const original = {
    ...snapshot(),
    rooms: [],
    private_access: { "@direct:bob": { id: "a" }, "@direct:eve": { id: "b" } },
    direct: [a, b],
  };
  const replacement = { ...message(3), private_id: "a" };
  const result = applyConversation(original, {
    view: "@direct:bob",
    id: "a",
    messages: [replacement],
    unread: { count: 1 },
  });
  assert.deepEqual(result.direct, [b, replacement]);
  assert.deepEqual(original.direct, [a, b]);
});

test("deleting and replacing entries updates budgets without evicting unrelated views", () => {
  const cache = new HistoryCache();
  for (let i = 0; i < 5; i++) cache.store(page(String(i), 1, 1000));
  cache.delete("0");
  cache.store(page("next", 1, 1000));
  assert.equal(cache.size, 5);
  assert.ok(cache.has("1"));
  cache.store(page("1", 1, 100));
  cache.store(page("small", 1, 900));
  assert.equal(cache.size, 6);
  assert.ok(cache.has("2"));
});
