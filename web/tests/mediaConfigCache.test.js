import assert from "node:assert/strict";
import test from "node:test";
import { mediaConfigCache } from "../src/api/mediaConfigCache.ts";

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const value = { enabled: true, max_size: 1024 };

test("media config shares reads with independent cancellation", async (t) => {
  mediaConfigCache.clear();
  t.after(() => mediaConfigCache.clear());
  const pending = deferred();
  let requests = 0,
    networkSignal;
  const fetch = (signal) => {
    requests++;
    networkSignal = signal;
    return pending.promise;
  };
  const a = new AbortController(),
    b = new AbortController();
  const first = mediaConfigCache.load("server", a.signal, fetch);
  const second = mediaConfigCache.load("server", b.signal, fetch);
  a.abort();
  await assert.rejects(first, { name: "AbortError" });
  assert.equal(networkSignal.aborted, false);
  pending.resolve(value);
  const result = await second;
  result.enabled = false;
  assert.deepEqual(
    await mediaConfigCache.load("server", b.signal, fetch),
    value,
  );
  assert.equal(
    requests,
    1,
    "composer, picker and remounts share configuration",
  );
});

test("last config reader aborts; late responses stay isolated", async (t) => {
  mediaConfigCache.clear();
  t.after(() => mediaConfigCache.clear());
  const pending = deferred();
  const controller = new AbortController();
  let networkSignal;
  const first = mediaConfigCache.load("server", controller.signal, (signal) => {
    networkSignal = signal;
    return pending.promise;
  });
  controller.abort();
  await assert.rejects(first, { name: "AbortError" });
  assert.equal(networkSignal.aborted, true);
  const fresh = { enabled: false, max_size: 2048 };
  assert.deepEqual(
    await mediaConfigCache.load(
      "server",
      new AbortController().signal,
      async () => fresh,
    ),
    fresh,
  );
  pending.resolve(value);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(
    await mediaConfigCache.load(
      "server",
      new AbortController().signal,
      async () => value,
    ),
    fresh,
  );
});

test("config reloads after context changes and failed reads", async (t) => {
  mediaConfigCache.clear();
  t.after(() => mediaConfigCache.clear());
  let requests = 0;
  const fetch = async () => {
    requests++;
    return value;
  };
  const signal = new AbortController().signal;
  mediaConfigCache.observeInstance("a", "first");
  await mediaConfigCache.load("a", signal, fetch);
  mediaConfigCache.observeInstance("a", "first");
  await mediaConfigCache.load("a", signal, fetch);
  assert.equal(requests, 1);
  mediaConfigCache.observeInstance("a", "restarted");
  await mediaConfigCache.load("a", signal, fetch);
  assert.equal(requests, 2);
  await mediaConfigCache.load("b", signal, fetch);
  assert.equal(requests, 3);
  mediaConfigCache.clear();
  await assert.rejects(
    mediaConfigCache.load("b", signal, async () => {
      throw new Error("offline");
    }),
    /offline/,
  );
  await mediaConfigCache.load("b", signal, fetch);
  assert.equal(requests, 4);
});
