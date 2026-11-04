import assert from "node:assert/strict";
import test from "node:test";
import { api } from "../src/api/client.ts";
import { saveServerUrl } from "../src/api/server.ts";
import { createHookHarness, mockHooks, useHarness } from "./helpers/hooks.js";
const registration = mockHooks();
const { useLibraryPage } = await import("../src/hooks/useLibraryPage.ts");
registration.deregister();
const events = new EventTarget();
const page = {
  items: [],
  next_cursor: null,
  usage: 12,
  quota: 100,
  cleanup_bytes: 0,
  cleanup_objects: 0,
  oldest_cleanup: null,
  uploads_paused: false,
};

function fixture(t) {
  const original = Object.fromEntries(
    ["window", "document", "localStorage"].map((key) => [
      key,
      Object.getOwnPropertyDescriptor(globalThis, key),
    ]),
  );
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: events,
  });
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: { baseURI: "http://library.test/" },
  });
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: () => null, setItem() {} },
  });
  saveServerUrl("http://library.test/");
  events.dispatchEvent(new Event("commonroom-library-change"));
  const requests = [],
    readers = [];
  t.mock.method(
    api,
    "library",
    (view, after, signal) =>
      new Promise((resolve, reject) => {
        requests.push({ view, after, signal, resolve, reject });
        signal.addEventListener(
          "abort",
          () => reject(new DOMException("Cancelled", "AbortError")),
          { once: true },
        );
      }),
  );
  function reader(view = "library", options = {}) {
    const harness = createHookHarness();
    let current;
    const instance = {
      render() {
        useHarness(harness);
        harness.dirty = false;
        current = useLibraryPage(view, { cacheScope: t.name, ...options });
        while (harness.effects.length) harness.effects.shift()();
        return current;
      },
      get current() {
        return current;
      },
      close() {
        for (const slot of harness.slots) slot?.cleanup?.();
      },
    };
    readers.push(instance);
    return instance;
  }
  t.after(() => {
    for (const item of readers) item.close();
    for (const [key, descriptor] of Object.entries(original)) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  const settle = async () => {
    await new Promise((resolve) => setImmediate(resolve));
  };
  return { requests, reader, settle };
}

test("library rerenders start only one GET", async (t) => {
  const f = fixture(t);
  const reader = f.reader();
  reader.render();
  reader.render();
  assert.equal(f.requests.length, 1);
  f.requests[0].resolve(page);
  await f.settle();
  assert.deepEqual(reader.render().page, page);
  assert.equal(f.requests.length, 1);
});

test("library readers share GETs and cancel independently", async (t) => {
  const f = fixture(t);
  const picker = f.reader("library", { retainWhileRefreshing: true });
  const quota = f.reader("library");
  picker.render();
  quota.render();
  assert.equal(
    f.requests.length,
    1,
    "identical page reads must share network work",
  );
  picker.close();
  assert.equal(
    f.requests[0].signal.aborted,
    false,
    "another subscriber still needs the page",
  );
  f.requests[0].resolve(page);
  await f.settle();
  assert.deepEqual(quota.render().page, page);
});

test("library pages isolate view, cursor and account", (t) => {
  const f = fixture(t);
  for (const reader of [
    f.reader(),
    f.reader("recent"),
    f.reader("library", { after: "next" }),
    f.reader("library", { cacheScope: "other-account" }),
  ])
    reader.render();
  assert.equal(f.requests.length, 4);
});

test("library invalidation replaces shared stale work once", async (t) => {
  const f = fixture(t);
  const one = f.reader(),
    two = f.reader();
  one.render();
  two.render();
  assert.equal(f.requests.length, 1);
  events.dispatchEvent(new Event("commonroom-library-change"));
  one.render();
  two.render();
  assert.equal(
    f.requests.length,
    2,
    "one new GET covers both refreshed readers",
  );
  assert.equal(f.requests[0].signal.aborted, true);
  f.requests[0].resolve({ ...page, usage: 1 });
  f.requests[1].resolve({ ...page, usage: 20 });
  await f.settle();
  assert.equal(one.render().page.usage, 20);
  assert.equal(two.render().page.usage, 20);
});

test("last reader cancels; later mounts retry", (t) => {
  const f = fixture(t);
  const old = f.reader();
  old.render();
  old.close();
  assert.equal(f.requests[0].signal.aborted, true);
  const next = f.reader();
  next.render();
  assert.equal(f.requests.length, 2);
  assert.equal(f.requests[1].signal.aborted, false);
});

test("fresh mounts reuse pages; refresh and expiry reload", async (t) => {
  const f = fixture(t);
  const initial = f.reader();
  initial.render();
  f.requests[0].resolve(page);
  await f.settle();
  initial.close();
  for (let i = 0; i < 3; i++) {
    const warm = f.reader();
    assert.deepEqual(warm.render().page, page);
    assert.equal(
      f.requests.length,
      1,
      "reopening a fresh page must not revalidate",
    );
    warm.close();
  }
  const refresh = f.reader();
  refresh.render().refresh();
  refresh.render();
  assert.equal(f.requests.length, 2, "manual refresh bypasses a fresh cache");
  f.requests[1].resolve({ ...page, usage: 20 });
  await f.settle();
  refresh.close();
  const now = Date.now();
  t.mock.method(Date, "now", () => now + 60001);
  const expired = f.reader();
  expired.render();
  assert.equal(f.requests.length, 3, "expired pages revalidate on mount");
  f.requests[2].resolve(page);
  await f.settle();
});
