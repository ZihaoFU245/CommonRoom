import assert from "node:assert/strict";
import test from "node:test";
import { BotCommandCache } from "../src/features/conversation/botCommandCache.ts";
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
    (id, signal) =>
      new Promise((resolve, reject) =>
        requests.push({ id, signal, resolve, reject }),
      ),
    () => {},
  );
  return { cache, requests };
}
test("slow refresh and transient errors keep cached completions, denials remove them", async (t) => {
  const originalNow = Date.now;
  let now = originalNow();
  Date.now = () => now;
  t.after(() => {
    Date.now = originalNow;
  });
  const { cache, requests } = fixture();
  cache.refresh("room", "members", 0);
  requests[0].resolve({ conversation_id: "room", bots });
  await settle();
  cache.refresh("room", "members", 1);
  assert.deepEqual(cache.get("room", "members"), bots);
  requests[1].reject(new Error("slow network"));
  await settle();
  assert.deepEqual(cache.get("room", "members"), bots);
  assert.equal(cache.get("room", "new members"), undefined);
  now += 30_000;
  cache.refresh("room", "members", 1);
  requests[2].reject(new ApiError(403, "revoked"));
  await settle();
  assert.equal(cache.get("room", "members"), undefined);
  cache.stop();
});
test("prefetch is bounded and stale responses cannot overwrite a new catalogue", async () => {
  const { cache, requests } = fixture();
  for (let i = 0; i < 10; i++) cache.refresh(String(i), "members", 0);
  assert.equal(requests.length, 4);
  cache.refresh("0", "members", 1);
  requests[0].resolve({ conversation_id: "0", bots });
  await settle();
  assert.equal(cache.get("0", "members"), undefined);
  assert.equal(requests.length, 5);
  cache.retain(new Set(["1"]));
  assert.ok(requests[1].signal.aborted === false);
  cache.stop();
  requests[1].resolve({ conversation_id: "1", bots });
  await settle();
  assert.equal(cache.get("1", "members"), undefined);
});
test("visited conversations remain immediately available without another read", async () => {
  const { cache, requests } = fixture();
  for (const id of ["a", "b"]) {
    cache.refresh(id, "members", 0);
    requests.at(-1).resolve({ conversation_id: id, bots });
    await settle();
  }
  cache.refresh("a", "members", 0);
  assert.equal(requests.length, 2);
  assert.deepEqual(cache.get("a", "members"), bots);
  cache.stop();
});

test("selecting a cold conversation jumps ahead of background prefetch", async () => {
  const { cache, requests } = fixture();
  for (let i = 0; i < 10; i++) cache.refresh(String(i), "members", 0);
  cache.refresh("9", "members", 0, false, true);
  requests[0].resolve({ conversation_id: "0", bots });
  await settle();
  assert.equal(requests[4].id, "9");
  cache.stop();
});

test("offline errors back off and empty results resist focus storms while notifications refresh immediately", async (t) => {
  const originalNow = Date.now;
  let now = originalNow();
  Date.now = () => now;
  t.after(() => {
    Date.now = originalNow;
  });
  const { cache, requests } = fixture();
  cache.refresh("room", "members", 0);
  requests[0].reject(new Error("offline"));
  await settle();
  for (let i = 0; i < 20; i++) cache.refresh("room", "members", 0, true);
  assert.equal(requests.length, 1);
  now += 30_000;
  cache.refresh("room", "members", 0);
  requests[1].reject(new Error("still offline"));
  await settle();
  now += 30_000;
  cache.refresh("room", "members", 0, true);
  assert.equal(requests.length, 2);
  now += 30_000;
  cache.refresh("room", "members", 0);
  requests[2].resolve({ conversation_id: "room", bots: [] });
  await settle();
  for (let i = 0; i < 20; i++) cache.refresh("room", "members", 0, true);
  assert.equal(requests.length, 3);
  cache.refresh("room", "members", 1);
  assert.equal(requests.length, 4);
  requests[3].resolve({ conversation_id: "room", bots });
  await settle();
  assert.deepEqual(cache.get("room", "members"), bots);
  cache.stop();
});
