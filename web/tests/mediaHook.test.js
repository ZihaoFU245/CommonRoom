import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";
import { api } from "../src/api/client.ts";

let index = 0;
const slots = [],
  effects = [];
globalThis.mediaTestHooks = {
  ref(value) {
    return (slots[index++] ??= { current: value });
  },
  state(value) {
    const slot = index++;
    if (!(slot in slots)) slots[slot] = value;
    return [
      slots[slot],
      (next) => {
        slots[slot] = next;
      },
    ];
  },
  effect(callback, deps) {
    const slot = index++,
      old = slots[slot];
    if (!old || deps.some((value, i) => !Object.is(value, old.deps[i])))
      effects.push(() => {
        old?.cleanup?.();
        slots[slot] = { deps, cleanup: callback() };
      });
  },
};
const registration = registerHooks({
  resolve(specifier, context, next) {
    return specifier === "preact/hooks"
      ? { url: "test:media-hooks", shortCircuit: true }
      : next(specifier, context);
  },
  load(url, context, next) {
    return url === "test:media-hooks"
      ? {
          format: "module",
          shortCircuit: true,
          source:
            "export const useRef=(...a)=>globalThis.mediaTestHooks.ref(...a); export const useState=(...a)=>globalThis.mediaTestHooks.state(...a); export const useEffect=(...a)=>globalThis.mediaTestHooks.effect(...a); export const useLayoutEffect=useEffect;",
        }
      : next(url, context);
  },
});
const { useMediaUploads } = await import("../src/hooks/useMediaUploads.ts");
registration.deregister();

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
    index = 0;
    const hook = useMediaUploads(conversation);
    while (effects.length) effects.shift()();
    return hook;
  }
  let hook = render("room-a");
  await Promise.resolve();
  hook = render("room-a");
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
    index = 0;
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
  assert.match(hook.error, /413/);
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
});
