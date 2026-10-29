import assert from "node:assert/strict";
import test from "node:test";
import { responseJson } from "../src/api/responseJson.ts";
import { MAX_JSON_BYTES, parseFrame } from "../src/api/protocol.ts";

test("JSON reads preserve split Unicode and reject invalid JSON", async () => {
  const bytes = new TextEncoder().encode(JSON.stringify({ text: "你好👋" }));
  const response = new Response(
    new ReadableStream({
      start(controller) {
        for (const byte of bytes) controller.enqueue(Uint8Array.of(byte));
        controller.close();
      },
    }),
  );
  assert.deepEqual(await responseJson(response), { text: "你好👋" });
  assert.equal(await responseJson(new Response("bad gateway")), null);
});

test("JSON byte limits cancel declared and chunked oversized bodies before parsing", async () => {
  let cancelled = 0;
  const body = () =>
    new ReadableStream({
      cancel() {
        cancelled++;
      },
    });
  await assert.rejects(
    responseJson(
      new Response(body(), {
        headers: { "Content-Length": String(MAX_JSON_BYTES + 1) },
      }),
    ),
    /size limit/,
  );
  assert.equal(cancelled, 1);
  const response = new Response(
    new ReadableStream({
      pull(controller) {
        controller.enqueue(new Uint8Array(1024 * 1024));
      },
      cancel() {
        cancelled++;
      },
    }),
  );
  await assert.rejects(responseJson(response), /size limit/);
  assert.equal(cancelled, 2);
  assert.equal(parseFrame(" ".repeat(MAX_JSON_BYTES + 1)), null);
});
