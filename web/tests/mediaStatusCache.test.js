import { mediaResolution } from "./helpers/mediaResolve.js";
import assert from "node:assert/strict";
import test from "node:test";
import { api } from "../src/api/client.ts";
import { saveServerUrl } from "../src/api/server.ts";
import { mediaStatusCache } from "../src/features/conversation/mediaStatusCache.ts";

const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
const info = {
  media_id: id,
  path: `/chat/api/media/${id}/file`,
  filename: "photo.png",
  content_type: "image/png",
  size: 6,
  expires: null,
  expired: false,
};

function setup(t) {
  const document = Object.getOwnPropertyDescriptor(globalThis, "document");
  const storage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "document", {
    value: { baseURI: "http://ui.test/chat/" },
    configurable: true,
  });
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem: () => null, setItem() {} },
    configurable: true,
  });
  t.after(() => {
    if (document) Object.defineProperty(globalThis, "document", document);
    else delete globalThis.document;
    if (storage) Object.defineProperty(globalThis, "localStorage", storage);
    else delete globalThis.localStorage;
  });
  mediaStatusCache.clear();
  saveServerUrl("http://api.test/chat/");
  t.after(() => mediaStatusCache.clear());
}

test("virtual remounts and duplicate attachments share metadata without transferring cancellation ownership", async (t) => {
  setup(t);
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  let calls = 0;
  let entered;
  const started = new Promise((resolve) => {
    entered = resolve;
  });
  t.mock.method(globalThis, "fetch", async (_url, options) => {
    calls++;
    entered();
    await gate;
    assert.equal(options.signal.aborted, false);
    return mediaResolution(_url, options, info);
  });
  const controller = new AbortController();
  const first = api.mediaStatus(id, controller.signal);
  controller.abort();
  const remounts = Array.from({ length: 20 }, () =>
    api.mediaStatus(id, new AbortController().signal),
  );
  await started;
  assert.equal(calls, 1);
  release();
  await first;
  for (const value of await Promise.all(remounts))
    assert.deepEqual(value, info);
  for (let i = 0; i < 20; i++)
    await api.mediaStatus(id, new AbortController().signal);
  assert.equal(calls, 1, "settled presentation metadata skips remount GETs");
  assert.deepEqual(api.cachedMediaStatus(id), info);
  const refresh = api.mediaStatus(id, new AbortController().signal, true);
  assert.deepEqual(
    api.cachedMediaStatus(id),
    info,
    "pending refreshes retain presentation metadata for new rows",
  );
  await refresh;
  assert.equal(calls, 2, "visibility/expiry refresh remains live");
});

test("metadata expires, stays bounded and cannot survive identity or deployment changes", async (t) => {
  setup(t);
  let now = 100000,
    calls = 0,
    status = 200;
  t.mock.method(Date, "now", () => now);
  t.mock.method(globalThis, "fetch", async (url, options) => {
    calls++;
    return String(url).includes("/resolve")
      ? mediaResolution(url, options, { ...info, expires: 101 }, status)
      : Response.json({ ok: true });
  });
  const load = (refresh = false) =>
    api.mediaStatus(id, new AbortController().signal, refresh);
  await load();
  now = 101000;
  assert.equal(api.cachedMediaStatus(id), undefined);
  await load();
  assert.equal(calls, 2);
  now = 100000;
  status = 403;
  await assert.rejects(load(true));
  assert.equal(api.cachedMediaStatus(id), undefined);
  status = 200;
  for (const reset of [
    () => api.logout(),
    () => api.login("alice", "test"),
    () => saveServerUrl("http://other.test/chat/"),
    () => saveServerUrl("http://api.test/chat/"),
  ]) {
    await load();
    assert.ok(api.cachedMediaStatus(id));
    await reset();
    assert.equal(api.cachedMediaStatus(id), undefined);
  }
  await load();
  for (let index = 0; index < 128; index++)
    await mediaStatusCache.load(`other-${index}`, async () => info);
  assert.equal(
    api.cachedMediaStatus(id),
    undefined,
    "least recent entry is evicted",
  );
  let complete;
  const pending = mediaStatusCache.load(
    "late",
    () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  );
  mediaStatusCache.clear();
  complete(info);
  await pending;
  assert.equal(
    mediaStatusCache.peek("late"),
    undefined,
    "late requests cannot repopulate a cleared session",
  );
});

test("pending metadata ownership is bounded and cleared work cannot repopulate it", async (t) => {
  setup(t);
  let release,
    calls = 0;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const fetchInfo = async () => {
    calls++;
    await gate;
    return info;
  };
  const loads = Array.from({ length: 128 }, (_, index) =>
    mediaStatusCache.load(`pending-${index}`, fetchInfo),
  );
  const duplicate = mediaStatusCache.load("pending-0", fetchInfo);
  const overflow = mediaStatusCache.load("overflow", fetchInfo);
  assert.equal(
    calls,
    129,
    "overflow evicts a record while duplicate callers share existing work",
  );
  mediaStatusCache.clear();
  release();
  await Promise.all([...loads, duplicate, overflow]);
  assert.equal(mediaStatusCache.peek("pending-0"), undefined);
  await mediaStatusCache.load("replacement", async () => info);
  assert.deepEqual(
    mediaStatusCache.peek("replacement"),
    info,
    "capacity recovers after clearing",
  );
});

test("library mutations invalidate cached membership and availability", async (t) => {
  setup(t);
  let saved = false;
  let unavailable = false;
  t.mock.method(globalThis, "fetch", async (url, options) => {
    if (String(url).includes("/resolve"))
      return mediaResolution(url, options, {
        ...info,
        library: true,
        saved,
        unavailable,
      });
    return Response.json({ ok: true });
  });
  const load = () => api.mediaStatus(id, new AbortController().signal);
  assert.equal((await load()).saved, false);
  saved = true;
  await api.libraryChange(id, "save", "room-id");
  assert.equal(api.cachedMediaStatus(id), undefined);
  assert.equal((await load()).saved, true);
  unavailable = true;
  await api.libraryChange(id, "release", undefined, true);
  assert.equal(api.cachedMediaStatus(id), undefined);
  assert.equal((await load()).unavailable, true);
});

test("forced refresh replaces pending metadata and late stale responses cannot overwrite it", async (t) => {
  setup(t);
  let stale;
  const previous = mediaStatusCache.load(
    "refresh",
    () =>
      new Promise((resolve) => {
        stale = resolve;
      }),
  );
  const fresh = { ...info, saved: true };
  assert.equal(
    await mediaStatusCache.load("refresh", async () => fresh, true),
    fresh,
  );
  stale(info);
  await previous;
  assert.equal(mediaStatusCache.peek("refresh"), fresh);
});
