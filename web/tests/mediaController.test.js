import { mediaResolution } from "./helpers/mediaResolve.js";
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

test("metadata resolves directly when secure worker startup is unavailable", async (t) => {
  const media = await controller(t, {});
  let calls = 0;
  t.mock.method(globalThis, "fetch", async (url, options) => {
    calls++;
    assert.equal(JSON.parse(options.body).items[0].link, false);
    return mediaResolution(url, options);
  });
  const value = await media.resolveMediaRead(
    "https://api.test/chat/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file",
  );
  assert.equal(value.info.size, 6);
  assert.equal(calls, 1);
});

test("invalidation waits for control and clears the worker before resolving", async (t) => {
  const serviceWorker = new EventTarget();
  const messages = [];
  const worker = {
    postMessage(message, [port]) {
      if (message === "commonroom-media:claim") {
        serviceWorker.controller = worker;
        port.postMessage("commonroom-media:ready");
      } else {
        messages.push(message.kind);
        if (message.kind === "commonroom-media:clear")
          port.postMessage({ ok: true });
      }
      port.close();
    },
  };
  serviceWorker.register = async () => ({ active: worker });
  const media = await controller(t, { serviceWorker });
  media.startMediaCache(new URL("https://ui.test/chat/media-cache.js"));
  await media.invalidateMediaReads();
  assert.deepEqual(messages, ["commonroom-media:clear"]);
});

test("HTTP debug metadata and image bytes share queued resolution without Cache Storage", async (t) => {
  const media = await controller(t, {});
  globalThis.document.baseURI = "http://ui.test/chat/";
  const { httpImageCache } =
    await import("../src/features/conversation/httpImageCache.ts");
  httpImageCache.invalidate();
  let resolutions = 0,
    storage = 0;
  t.mock.method(globalThis, "fetch", async (url, options) => {
    if (String(url).endsWith("api/media/resolve")) {
      resolutions++;
      return mediaResolution(url, options);
    }
    storage++;
    return new Response("abcdef", { headers: { "Content-Length": "6" } });
  });
  const href =
    "http://api.test/chat/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file";
  const [info, blob] = await Promise.all([
    media.resolveMediaRead(href),
    httpImageCache.load(href, new AbortController().signal),
  ]);
  URL.revokeObjectURL(blob);
  assert.equal(info.info.size, 6);
  assert.equal(resolutions, 1);
  assert.equal(storage, 1);
  await media.invalidateMediaReads();
});

test("invalidation during startup retries metadata and clears the newly controlled worker", async (t) => {
  const serviceWorker = new EventTarget();
  let register;
  serviceWorker.register = () =>
    new Promise((resolve) => {
      register = resolve;
    });
  const media = await controller(t, { serviceWorker });
  media.startMediaCache(new URL("https://ui.test/chat/media-cache.js"));
  let finish, entered;
  const started = new Promise((resolve) => {
    entered = resolve;
  });
  let calls = 0,
    clears = 0;
  t.mock.method(globalThis, "fetch", (url, options) => {
    calls++;
    if (calls > 1)
      return Promise.resolve(
        mediaResolution(url, options, { filename: "fresh" }),
      );
    return new Promise((resolve) => {
      finish = () =>
        resolve(mediaResolution(url, options, { filename: "stale" }));
      entered();
    });
  });
  const href =
    "https://api.test/chat/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file";
  const read = media.resolveMediaRead(href);
  await started;
  const cleared = media.invalidateMediaReads();
  finish();
  assert.equal((await read).info.filename, "fresh");
  const worker = {
    postMessage(message, [port]) {
      if (message === "commonroom-media:claim") {
        serviceWorker.controller = worker;
        port.postMessage("commonroom-media:ready");
      } else {
        assert.equal(message.kind, "commonroom-media:clear");
        clears++;
        port.postMessage({ ok: true });
      }
      port.close();
    },
  };
  register({ active: worker });
  await cleared;
  assert.equal(calls, 2);
  assert.equal(clears, 1);
});

test("worker replacement keeps an in-flight backend and never transfers its lease", async (t) => {
  const serviceWorker = new EventTarget();
  let active, reply, entered;
  const started = new Promise((resolve) => {
    entered = resolve;
  });
  serviceWorker.register = async () => ({ active });
  const media = await controller(t, { serviceWorker });
  const href =
    "https://api.test/chat/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file";
  const packet = await mediaResolution(
    new URL("api/media/resolve", "https://api.test/chat/"),
    {
      body: JSON.stringify({
        items: [{ kind: "media", id: href.split("/").at(-2) }],
      }),
    },
  ).json();
  const value = {
    info: packet.items[0].info,
    expiresAt: Date.now() + 900000,
    renewAt: Date.now() + 720000,
    linkExpiresAt: 0,
  };
  const requests = [];
  const worker = (name, delay = false) => ({
    postMessage(message, [port]) {
      if (message === "commonroom-media:claim") {
        serviceWorker.controller = active;
        port.postMessage("commonroom-media:ready");
        port.close();
      } else {
        requests.push(name);
        const respond = () => {
          port.postMessage({ value });
          port.close();
        };
        if (delay) {
          reply = respond;
          entered();
        } else respond();
      }
    },
  });
  active = worker("first", true);
  media.startMediaCache(new URL("https://ui.test/chat/media-cache.js"));
  await media.ensureMediaCache();
  const first = media.resolveMediaRead(href);
  await started;
  active = worker("replacement");
  serviceWorker.controller = active;
  assert.equal(media.mediaCacheReady(), false);
  let fallback = 0;
  t.mock.method(globalThis, "fetch", (url, options) => {
    fallback++;
    return Promise.resolve(mediaResolution(url, options));
  });
  await media.resolveMediaRead(href);
  reply();
  await first;
  await media.ensureMediaCache();
  await media.resolveMediaRead(href);
  assert.equal(fallback, 1);
  assert.deepEqual(requests, ["first", "replacement"]);
});
