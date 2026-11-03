import assert from "node:assert/strict";
import test from "node:test";

// Each document owns one startup promise; fresh modules represent fresh pages.
async function controller(t, navigator) {
  for (const [key, value] of Object.entries({
    navigator,
    document: { baseURI: "https://ui.test/chat/" },
  })) {
    const descriptor = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, { value, configurable: true });
    t.after(() => {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    });
  }
  return import(`../src/features/conversation/mediaController.ts?${t.name}`);
}

test("cache initialization rejects insecure contexts and registration failures without a network bypass", async (t) => {
  const media = await controller(t, {});
  media.startMediaCache(new URL("https://ui.test/chat/media-cache.js"));
  assert.equal(media.mediaCacheReady(), false);
  await assert.rejects(media.ensureMediaCache(), /HTTPS or localhost/u);
  const serviceWorker = new EventTarget();
  serviceWorker.register = () => {
    throw new Error("Worker registration denied");
  };
  globalThis.navigator.serviceWorker = serviceWorker;
  await assert.rejects(media.ensureMediaCache(), /Worker registration denied/u);
});

test("stalled startup has a bounded deadline, releases listeners and can retry on Chrome 114 APIs", async (t) => {
  t.mock.method(AbortSignal, "any", undefined);
  t.mock.method(AbortSignal, "timeout", undefined);
  const serviceWorker = new EventTarget();
  let registrations = 0;
  serviceWorker.register = () => {
    registrations++;
    return new Promise(() => {});
  };
  const added = t.mock.method(serviceWorker, "addEventListener");
  const removed = t.mock.method(serviceWorker, "removeEventListener");
  let deadline;
  t.mock.method(globalThis, "setTimeout", (callback, delay) => {
    assert.equal(delay, 15000);
    deadline = callback;
    return 1;
  });
  t.mock.method(globalThis, "clearTimeout", () => {});
  const media = await controller(t, { serviceWorker });
  media.startMediaCache(new URL("https://ui.test/chat/media-cache.js"));
  const pending = media.ensureMediaCache();
  assert.equal(
    media.ensureMediaCache(),
    pending,
    "concurrent controls share startup",
  );
  const failed = assert.rejects(pending, /Media cache could not start/u);
  deadline();
  await failed;
  assert.equal(added.mock.callCount(), 1);
  assert.equal(removed.mock.callCount(), 1);
  const worker = {
    postMessage(message, [port]) {
      assert.equal(message, "commonroom-media:claim");
      serviceWorker.controller = worker;
      port.postMessage("commonroom-media:ready");
      port.close();
    },
  };
  serviceWorker.register = async (url, options) => {
    registrations++;
    assert.equal(url.href, "https://ui.test/chat/media-cache.js");
    assert.deepEqual(options, {
      type: "module",
      scope: "/chat/",
      updateViaCache: "none",
    });
    return { active: worker };
  };
  await media.ensureMediaCache();
  assert.equal(registrations, 2);
  assert.equal(removed.mock.callCount(), 2);
  assert.equal(media.mediaCacheReady(), true);
  serviceWorker.controller = null;
  assert.equal(
    media.mediaCacheReady(),
    false,
    "lost controller invalidates synchronous readiness",
  );
  await media.ensureMediaCache();
  assert.equal(registrations, 3);
  assert.equal(media.mediaCacheReady(), true);
});
