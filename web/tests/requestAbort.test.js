import assert from "node:assert/strict";
import { getEventListeners } from "node:events";
import test from "node:test";
import { requestAbort } from "../src/api/requestAbort.ts";
import { api } from "../src/api/client.ts";
import { documentSource } from "../src/api/media.ts";

function withoutAny(t) {
  const descriptor = Object.getOwnPropertyDescriptor(AbortSignal, "any");
  Object.defineProperty(AbortSignal, "any", {
    configurable: true,
    value: undefined,
  });
  t.after(() => {
    if (descriptor) Object.defineProperty(AbortSignal, "any", descriptor);
    else delete AbortSignal.any;
  });
}

for (const native of [true, false]) {
  test(`request cancellation preserves reasons and disposes timers (native=${native})`, (t) => {
    if (!native) withoutAny(t);
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const parent = new AbortController();
    const cancellation = requestAbort(100, parent.signal);
    assert.equal(cancellation.signal.aborted, false);
    const reason = new DOMException("Navigation", "AbortError");
    parent.abort(reason);
    assert.equal(cancellation.signal.reason, reason);
    t.mock.timers.tick(200);
    assert.equal(cancellation.signal.reason, reason);
    assert.equal(getEventListeners(parent.signal, "abort").length, 0);
    cancellation.dispose();
    cancellation.dispose();
  });

  test(`timeouts use TimeoutError and release parent listeners (native=${native})`, (t) => {
    if (!native) withoutAny(t);
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const parent = new AbortController();
    const cancellation = requestAbort(100, parent.signal);
    t.mock.timers.tick(99);
    assert.equal(cancellation.signal.aborted, false);
    t.mock.timers.tick(1);
    assert.equal(cancellation.signal.reason.name, "TimeoutError");
    parent.abort(new Error("too late"));
    assert.equal(cancellation.signal.reason.name, "TimeoutError");
    assert.equal(getEventListeners(parent.signal, "abort").length, 0);
    cancellation.dispose();
  });

  test(`already-aborted parents and successful completion leave no timer (native=${native})`, (t) => {
    if (!native) withoutAny(t);
    t.mock.timers.enable({ apis: ["setTimeout"] });
    const reason = new Error("already cancelled");
    const parent = new AbortController();
    parent.abort(reason);
    const aborted = requestAbort(100, parent.signal);
    assert.equal(aborted.signal.reason, reason);
    aborted.dispose();
    const completed = requestAbort(100);
    completed.dispose();
    t.mock.timers.tick(200);
    assert.equal(completed.signal.aborted, false);
    assert.equal(getEventListeners(parent.signal, "abort").length, 0);
  });
}

test("fallback completion detaches listeners from a long-lived parent", (t) => {
  withoutAny(t);
  const parent = new AbortController();
  for (let i = 0; i < 20; i++) {
    const request = requestAbort(15000, parent.signal);
    assert.equal(getEventListeners(parent.signal, "abort").length, 1);
    request.dispose();
    assert.equal(getEventListeners(parent.signal, "abort").length, 0);
  }
});

test("HTTP and preview requests work without AbortSignal.any and clean up on all exits", async (t) => {
  withoutAny(t);
  const originalDocument = globalThis.document;
  globalThis.document = { baseURI: "https://chat.test/" };
  t.after(() => {
    if (originalDocument === undefined) delete globalThis.document;
    else globalThis.document = originalDocument;
  });
  const parent = new AbortController();
  t.mock.method(globalThis, "fetch", async () =>
    Response.json({ enabled: true, max_size: 1024 }),
  );
  assert.equal((await api.mediaConfig(parent.signal)).enabled, true);
  assert.equal(getEventListeners(parent.signal, "abort").length, 0);
  t.mock.method(globalThis, "fetch", async () => new Response("preview"));
  assert.equal(await documentSource("/file", parent.signal), "preview");
  assert.equal(getEventListeners(parent.signal, "abort").length, 0);
  t.mock.method(
    globalThis,
    "fetch",
    async () => new Response("failure", { status: 503 }),
  );
  await assert.rejects(api.mediaConfig(parent.signal), /503/);
  await assert.rejects(
    documentSource("/file", parent.signal),
    /Unable to load/,
  );
  assert.equal(getEventListeners(parent.signal, "abort").length, 0);
  t.mock.method(globalThis, "fetch", async () => {
    throw new TypeError("offline");
  });
  await assert.rejects(api.mediaConfig(parent.signal), /offline/);
  await assert.rejects(documentSource("/file", parent.signal), /offline/);
  assert.equal(getEventListeners(parent.signal, "abort").length, 0);
});

test("request deadlines remain active through JSON and preview body reads", async (t) => {
  withoutAny(t);
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const originalDocument = globalThis.document;
  globalThis.document = { baseURI: "https://chat.test/" };
  t.after(() => {
    if (originalDocument === undefined) delete globalThis.document;
    else globalThis.document = originalDocument;
  });
  const parent = new AbortController();
  let bodyStarted;
  let started = new Promise((resolve) => {
    bodyStarted = resolve;
  });
  t.mock.method(
    globalThis,
    "fetch",
    async (_url, options) =>
      new Response(
        new ReadableStream({
          start(controller) {
            options.signal.addEventListener(
              "abort",
              () => controller.error(options.signal.reason),
              { once: true },
            );
            bodyStarted();
          },
        }),
      ),
  );
  const pending = api.mediaConfig(parent.signal);
  await started;
  t.mock.timers.tick(15000);
  await assert.rejects(pending, { name: "TimeoutError" });
  assert.equal(getEventListeners(parent.signal, "abort").length, 0);
  started = new Promise((resolve) => {
    bodyStarted = resolve;
  });
  t.mock.method(
    globalThis,
    "fetch",
    async (_url, options) =>
      new Response(
        new ReadableStream({
          start(controller) {
            options.signal.addEventListener(
              "abort",
              () => controller.error(options.signal.reason),
              { once: true },
            );
            bodyStarted();
          },
        }),
      ),
  );
  const preview = documentSource("/file", parent.signal);
  await started;
  t.mock.timers.tick(15000);
  await assert.rejects(preview, { name: "TimeoutError" });
  assert.equal(getEventListeners(parent.signal, "abort").length, 0);
});
