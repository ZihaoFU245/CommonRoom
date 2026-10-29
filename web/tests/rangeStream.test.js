import assert from "node:assert/strict";
import test from "node:test";
import { rangeStream } from "../src/features/conversation/rangeStream.ts";

test("cached ranges stream across chunks and cancel without reading the rest of the file", async () => {
  let chunks = 0,
    cancelled = false;
  const body = new ReadableStream({
    pull(controller) {
      chunks++;
      controller.enqueue(new TextEncoder().encode("abcdef"));
    },
    cancel() {
      cancelled = true;
    },
  });
  const result = new Response(
    rangeStream(body, 4, 8, new AbortController().signal),
  );
  assert.equal(await result.text(), "efabc");
  assert.ok(chunks <= 3);
  assert.equal(cancelled, true);
});

test("range cancellation and abort release cached readers", async () => {
  let cancelled = false;
  const body = new ReadableStream({
    cancel() {
      cancelled = true;
    },
  });
  const abort = new AbortController();
  const reader = rangeStream(body, 0, 10, abort.signal).getReader();
  const pending = reader.read();
  abort.abort();
  await assert.rejects(pending, { name: "AbortError" });
  assert.equal(cancelled, true);
  let released = false;
  await rangeStream(
    new ReadableStream({
      cancel() {
        released = true;
      },
    }),
    0,
    10,
    new AbortController().signal,
  ).cancel();
  assert.equal(released, true);
});
