import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, mockHooks, useHarness } from "./helpers/hooks.js";
import { api } from "../src/api/client.ts";
import { httpImageCache } from "../src/features/conversation/httpImageCache.ts";
import {
  startMediaCache,
  ensureMediaCache,
} from "../src/features/conversation/mediaController.ts";
import { saveServerUrl, serverBaseUrl } from "../src/api/server.ts";

const harness = createHookHarness();
const { slots, effects } = harness;
const registration = mockHooks();
const { useMediaUploads } = await import("../src/hooks/useMediaUploads.ts");
const { useFileUrl } = await import("../src/hooks/useFileUrl.ts");
registration.deregister();

test("LAN HTTP resolves fresh media, avatar and download links without requiring a worker", async (t) => {
  const descriptors = Object.fromEntries(
    ["document", "navigator", "localStorage"].map((key) => [
      key,
      Object.getOwnPropertyDescriptor(globalThis, key),
    ]),
  );
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      baseURI: "http://ui.test/chat/",
      hidden: true,
      addEventListener() {},
      removeEventListener() {},
    },
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {},
  });
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: () => null, setItem() {} },
  });
  const originalBase = serverBaseUrl();
  saveServerUrl("http://api.test/server/");
  const local = createHookHarness();
  t.after(() => {
    for (const slot of local.slots) slot?.cleanup?.();
    saveServerUrl(originalBase);
    for (const [key, descriptor] of Object.entries(descriptors)) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  const calls = [];
  let denied = false;
  t.mock.method(api, "fileLink", async (href, signal, download) => {
    calls.push({ href, signal, download });
    if (denied) throw new Error("Attachment not found.");
    return {
      url: `http://store.test/object?signature=${calls.length}&download=${download}`,
    };
  });
  function draw(kind = "media", download = false, image = false) {
    useHarness(local);
    const result = useFileUrl(
      `http://api.test/server/api/${kind}/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file`,
      { download, image },
    );
    while (local.effects.length) local.effects.shift()();
    return result;
  }
  assert.equal(draw().url, undefined);
  await new Promise((resolve) => setImmediate(resolve));
  assert.match(draw().url, /^http:\/\/store.test\/object/u);
  assert.equal(draw().crossOrigin, "anonymous");
  draw().retry();
  draw();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 2, "retry resolves a fresh signed URL");
  assert.equal(draw("avatars").url, undefined);
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(draw("avatars").url);
  draw("media", true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.at(-1).download, true);
  assert.match(draw("media", true).url, /download=true/u);
  denied = true;
  draw("media", true).retry();
  draw("media", true);
  await new Promise((resolve) => setImmediate(resolve));
  const rejected = draw("media", true);
  assert.equal(rejected.url, undefined);
  assert.equal(rejected.error, "Attachment not found.");
  const revoke = t.mock.method(URL, "revokeObjectURL");
  let finish;
  t.mock.method(
    httpImageCache,
    "load",
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  draw("media", false, true);
  const imageUrl = URL.createObjectURL(new Blob(["image"]));
  finish(imageUrl);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(draw("media", false, true).url, imageUrl);
  draw("avatars", false, true);
  assert.ok(
    revoke.mock.calls.some(({ arguments: args }) => args[0] === imageUrl),
    "changing rows releases their object URLs while keeping shared bytes",
  );
  const late = URL.createObjectURL(new Blob(["late image"]));
  draw("avatars", false, false);
  finish(late);
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(
    revoke.mock.calls.some(({ arguments: args }) => args[0] === late),
    "a late image cannot leak a URL after its consumer unmounts",
  );
});

test("managed file hooks wait for cache control and never resolve direct storage links", async (t) => {
  const descriptors = Object.fromEntries(
    ["document", "navigator", "localStorage"].map((key) => [
      key,
      Object.getOwnPropertyDescriptor(globalThis, key),
    ]),
  );
  const serviceWorker = new EventTarget();
  serviceWorker.controller = null;
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: {
      baseURI: "https://ui.test/chat/",
      hidden: true,
      addEventListener() {},
      removeEventListener() {},
    },
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { serviceWorker },
  });
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: () => null, setItem() {} },
  });
  const originalBase = serverBaseUrl();
  saveServerUrl("https://api.test/server/");
  let local = createHookHarness();
  let release,
    registrations = 0;
  const worker = {
    postMessage(message, [port]) {
      if (message.kind === "commonroom-media:clear") {
        port.postMessage({ ok: true });
        port.close();
        return;
      }
      assert.equal(message, "commonroom-media:claim");
      if (serviceWorker.controller) {
        port.postMessage("commonroom-media:ready");
        port.close();
      } else
        release = () => {
          serviceWorker.controller = worker;
          serviceWorker.dispatchEvent(new Event("controllerchange"));
          port.postMessage("commonroom-media:ready");
          port.close();
        };
    },
  };
  serviceWorker.register = async () => {
    registrations++;
    return { active: worker };
  };
  const href =
    "https://api.test/server/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file";
  function draw(source = href) {
    useHarness(local);
    const result = useFileUrl(source);
    while (local.effects.length) local.effects.shift()();
    return result;
  }
  t.after(() => {
    release?.();
    for (const slot of local.slots) slot?.cleanup?.();
    saveServerUrl(originalBase);
    for (const [key, descriptor] of Object.entries(descriptors)) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else delete globalThis[key];
    }
  });
  startMediaCache(new URL("https://ui.test/chat/media-cache.js"));
  assert.equal(draw().url, undefined);
  await Promise.resolve();
  assert.equal(
    draw().url,
    undefined,
    "uncontrolled pages cannot start an uncached load",
  );
  release();
  assert.equal(
    draw().url,
    undefined,
    "controller presence alone cannot bypass acknowledgment",
  );
  await ensureMediaCache();
  await Promise.resolve();
  assert.equal(
    registrations,
    1,
    "all mounts share one bounded startup handshake",
  );
  const controlled = draw();
  assert.equal(controlled.url, href);
  controlled.retry();
  assert.equal(new URL(draw().url).searchParams.get("retry"), "0:1");
  for (const slot of local.slots) slot?.cleanup?.();
  local = createHookHarness();
  useHarness(local);
  assert.equal(
    useFileUrl(href).url,
    href,
    "warm remount exposes its URL before effects run",
  );
  local.dirty = false;
  while (local.effects.length) local.effects.shift()();
  await Promise.resolve();
  assert.equal(
    local.dirty,
    false,
    "warm remount schedules no promise-driven state update",
  );
  saveServerUrl("https://ui.test/chat/");
  const localPath = "/chat/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file";
  const sameOrigin = draw(localPath);
  sameOrigin.retry();
  assert.equal(
    new URL(draw(localPath).url).searchParams.get("retry"),
    "0:1",
    "same-origin retries also issue a fresh URL",
  );
});

test("late completion never inserts into a replacement conversation; cancellation releases the composer", async (t) => {
  const originals = {
    config: api.mediaConfig,
    allocate: api.allocateMedia,
    complete: api.completeMedia,
    abandon: api.abandonMedia,
    xhr: globalThis.XMLHttpRequest,
    document: globalThis.document,
  };
  t.after(() => {
    for (const slot of slots) slot?.cleanup?.();
    api.mediaConfig = originals.config;
    api.allocateMedia = originals.allocate;
    api.completeMedia = originals.complete;
    api.abandonMedia = originals.abandon;
    globalThis.XMLHttpRequest = originals.xhr;
    globalThis.document = originals.document;
  });
  slots.length = 0;
  api.abandonMedia = async () => ({ ok: true });
  api.mediaConfig = async () => ({ enabled: true, max_size: 25 * 1024 * 1024 });
  const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
  api.allocateMedia = async () => ({
    media_id: id,
    upload: {
      method: "PUT",
      url: "https://storage.test/file",
      headers: { "Content-Type": "application/octet-stream" },
      expires_in: 300,
    },
  });
  let finish,
    completionStarted = false;
  api.completeMedia = () => {
    completionStarted = true;
    return new Promise((resolve) => {
      finish = resolve;
    });
  };
  globalThis.document = { baseURI: "https://chat.test/" };
  globalThis.XMLHttpRequest = class {
    upload = {};
    open() {}
    setRequestHeader() {}
    send() {
      this.status = 200;
      this.onload();
    }
    abort() {
      this.onabort();
    }
  };
  function render(conversation) {
    useHarness(harness);
    const hook = useMediaUploads(conversation);
    while (effects.length) effects.shift()();
    return hook;
  }
  render("room-a");
  await Promise.resolve();
  let hook = render("room-a");
  const pending = hook.start(new File(["test"], "test.png"));
  for (let i = 0; i < 10 && !completionStarted; i++) await Promise.resolve();
  assert.ok(completionStarted);
  hook = render("room-b");
  finish({
    media_id: id,
    path: `/api/media/${id}/file`,
    filename: "test.png",
    content_type: "image/png",
    size: 4,
  });
  await pending;
  assert.equal(hook.attachment, null);
  hook = render("room-b");
  assert.equal(hook.busy, false);
  const cancelled = hook.start(new File(["test"], "test.png"));
  hook.cancel();
  await cancelled;
  hook = render("room-b");
  assert.equal(hook.busy, false);
  assert.equal(hook.attachment, null);
});

test("verified attachments stay separate from text and release only on acknowledgement or removal", async (t) => {
  const original = {
    config: api.mediaConfig,
    allocate: api.allocateMedia,
    complete: api.completeMedia,
    abandon: api.abandonMedia,
    xhr: globalThis.XMLHttpRequest,
  };
  t.after(() => {
    for (const slot of slots) slot?.cleanup?.();
    api.mediaConfig = original.config;
    api.allocateMedia = original.allocate;
    api.completeMedia = original.complete;
    api.abandonMedia = original.abandon;
    globalThis.XMLHttpRequest = original.xhr;
  });
  slots.length = 0;
  const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
  const removed = [];
  api.mediaConfig = async () => ({ enabled: true, max_size: 8 });
  api.allocateMedia = async () => ({
    media_id: id,
    upload: {
      method: "PUT",
      url: "https://storage.test/upload",
      headers: { "Content-Type": "application/octet-stream" },
      expires_in: 300,
    },
  });
  api.completeMedia = async () => ({
    media_id: id,
    path: `/api/media/${id}/file`,
    filename: "notes.txt",
    content_type: "application/octet-stream",
    size: 4,
  });
  api.abandonMedia = async (id) => {
    removed.push(id);
    return { ok: true };
  };
  let status = 200;
  globalThis.XMLHttpRequest = class {
    upload = {};
    open() {}
    setRequestHeader() {}
    send() {
      this.status = status;
      this.onload();
    }
    abort() {
      this.onabort();
    }
  };
  function render(instance = "server-a") {
    useHarness(harness);
    const hook = useMediaUploads("room", instance);
    while (effects.length) effects.shift()();
    return hook;
  }
  render();
  await Promise.resolve();
  let hook = render();
  await hook.start(new File(["123456789"], "too-large.txt"));
  hook = render();
  assert.match(hook.error, /8 bytes/);
  assert.equal(hook.attachment, null);
  await hook.start(new File(["test"], "notes.txt"));
  hook = render();
  assert.equal(hook.busy, false);
  assert.equal(hook.attachment.media.media_id, id);
  assert.equal(hook.attachment.preview, null);
  assert.deepEqual(removed, []);
  hook.published("another-id");
  hook = render();
  assert.ok(hook.attachment);
  hook.published(id);
  hook = render();
  assert.equal(hook.attachment, null);
  assert.deepEqual(removed, []);
  status = 413;
  await hook.start(new File(["test"], "notes.txt"));
  hook = render();
  assert.match(hook.error, /too large/);
  assert.equal(hook.busy, false);
  assert.equal(hook.attachment, null);
  assert.deepEqual(removed, [id]);
  status = 200;
  await hook.retry();
  hook = render();
  assert.ok(hook.attachment);
  hook.cancel();
  hook = render();
  assert.equal(hook.attachment, null);
  assert.deepEqual(removed, [id, id]);
  api.allocateMedia = async () => {
    throw new Error(
      "Upload rejected: media storage is unavailable. Try again later.",
    );
  };
  await hook.start(new File(["test"], "offline.txt"));
  hook = render();
  assert.equal(hook.enabled, true);
  assert.equal(hook.busy, false);
  assert.equal(hook.attachment, null);
  assert.match(hook.error, /Upload rejected/);
  assert.ok(hook.retry);
  api.mediaConfig = async () => ({ enabled: false, max_size: 8 });
  render("server-b");
  await Promise.resolve();
  hook = render("server-b");
  assert.equal(hook.enabled, false);
  api.mediaConfig = async () => ({ enabled: true, max_size: 8 });
  render("server-c");
  await Promise.resolve();
  hook = render("server-c");
  assert.equal(hook.enabled, true);
  api.mediaConfig = async () => ({ enabled: true, max_size: 6 * 1024 * 1024 });
  api.allocateMedia = async () => ({
    media_id: id,
    upload: {
      method: "PUT",
      url: "https://storage.test/upload",
      headers: { "Content-Type": "application/octet-stream" },
      expires_in: 300,
    },
  });
  api.completeMedia = async () => ({
    media_id: id,
    path: `/api/media/${id}/file`,
    filename: "large.png",
    content_type: "image/png",
    size: 5 * 1024 * 1024,
  });
  render("server-d");
  await Promise.resolve();
  hook = render("server-d");
  await hook.start(new File([new Uint8Array(5 * 1024 * 1024)], "large.png"));
  hook = render("server-d");
  assert.ok(hook.attachment);
  assert.equal(
    hook.attachment.preview,
    null,
    "Large images do not allocate a decoded composer preview",
  );
  render(null);
  render("server-e");
  hook = render("server-e");
  assert.equal(hook.attachment, null);
  assert.match(hook.error, /server restarted.*Attach the file again/);
  assert.equal(hook.busy, false);
  hook.cancel();
});
