import assert from "node:assert/strict";
import test from "node:test";
import {
  BotCommandCache,
  BOT_COMMAND_REFRESH_MS,
} from "../src/features/conversation/botCommandCache.ts";
import { ApiError } from "../src/api/client.ts";
const bots = [
  {
    account_id: "helper",
    username: "helper",
    commands: [{ name: "/ping", usage: "/ping", description: "Ping" }],
  },
];
const settle = () => new Promise((resolve) => setImmediate(resolve));
function fixture() {
  const requests = [];
  const cache = new BotCommandCache(
    (signal) =>
      new Promise((resolve, reject) =>
        requests.push({ signal, resolve, reject }),
      ),
    () => {},
  );
  return { cache, requests };
}
const directory = (...ids) => ({
  conversations: ids.map((conversation_id) => ({ conversation_id, bots })),
});
test("one request covers hundreds of conversations and selection needs no additional reads", async () => {
  const { cache, requests } = fixture();
  const rooms = new Map(
    Array.from({ length: 200 }, (_, i) => [String(i), "members"]),
  );
  cache.refresh(rooms, new Map());
  assert.equal(requests.length, 1);
  requests[0].resolve(directory(...rooms.keys()));
  await settle();
  for (const id of rooms.keys()) {
    assert.deepEqual(cache.get(id, "members"), bots);
    cache.refresh(new Map(rooms), new Map());
  }
  assert.equal(requests.length, 1);
  cache.stop();
});
test("refresh is cached for 120 seconds; catalogue changes invalidate the batch immediately", async (t) => {
  const original = Date.now;
  let now = original();
  Date.now = () => now;
  t.after(() => {
    Date.now = original;
  });
  const { cache, requests } = fixture();
  const rooms = new Map([
    ["a", "members"],
    ["b", "members"],
  ]);
  cache.refresh(rooms, new Map());
  requests[0].resolve(directory("a", "b"));
  await settle();
  now += BOT_COMMAND_REFRESH_MS - 1;
  cache.refresh(rooms, new Map());
  assert.equal(requests.length, 1);
  now++;
  cache.refresh(rooms, new Map());
  assert.equal(requests.length, 2);
  assert.deepEqual(cache.get("a", "members"), bots);
  requests[1].resolve(directory("a", "b"));
  await settle();
  cache.refresh(rooms, new Map([["a", 1]]));
  assert.equal(requests.length, 3);
  requests[2].resolve(directory("a"));
  await settle();
  assert.equal(cache.get("b", "members"), undefined);
  cache.stop();
});
test("stale batches cannot restore revoked conversations or overwrite new catalogues", async () => {
  const { cache, requests } = fixture();
  cache.refresh(new Map([["a", "old"]]), new Map());
  cache.refresh(new Map([["b", "new"]]), new Map());
  assert.ok(requests[0].signal.aborted);
  requests[1].resolve(directory("b"));
  await settle();
  requests[0].resolve(directory("a"));
  await settle();
  assert.equal(cache.get("a", "old"), undefined);
  assert.deepEqual(cache.get("b", "new"), bots);
  cache.stop();
});
test("network failures preserve cached suggestions and back off; authoritative denials clear them", async (t) => {
  const original = Date.now;
  let now = original();
  Date.now = () => now;
  t.after(() => {
    Date.now = original;
  });
  const { cache, requests } = fixture();
  const rooms = new Map([["a", "members"]]);
  cache.refresh(rooms, new Map());
  requests[0].resolve(directory("a"));
  await settle();
  const versions = new Map([["a", 1]]);
  cache.refresh(rooms, versions);
  requests[1].reject(new Error("offline"));
  await settle();
  for (let i = 0; i < 20; i++) cache.refresh(rooms, versions);
  assert.equal(requests.length, 2);
  assert.deepEqual(cache.get("a", "members"), bots);
  now += 30000;
  cache.refresh(rooms, versions);
  requests[2].reject(new ApiError(403, "revoked"));
  await settle();
  assert.equal(cache.get("a", "members"), undefined);
  cache.stop();
});

test("catalogue version changes cannot bypass an outage backoff", async (t) => {
  const original = Date.now;
  let now = original();
  Date.now = () => now;
  t.after(() => {
    Date.now = original;
  });
  const { cache, requests } = fixture();
  const rooms = new Map([["a", "members"]]);
  cache.refresh(rooms, new Map());
  requests[0].reject(new Error("offline"));
  await settle();
  for (let version = 1; version <= 20; version++) {
    cache.refresh(rooms, new Map([["a", version]]));
  }
  assert.equal(requests.length, 1);
  now += 30000;
  cache.refresh(rooms, new Map([["a", 21]]));
  assert.equal(requests.length, 2);
  cache.stop();
});
