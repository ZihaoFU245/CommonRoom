import assert from "node:assert/strict";
import test from "node:test";
import { api, sendFrame } from "../src/api/client.ts";

test("HTTP client preserves base paths, cookies, cancellation and validates responses", async (t) => {
  const originalFetch = globalThis.fetch,
    originalDocument = globalThis.document;
  t.after(() => {
    globalThis.fetch = originalFetch;
    if (originalDocument === undefined) delete globalThis.document;
    else globalThis.document = originalDocument;
  });
  globalThis.document = { baseURI: "https://example.com/commonroom/" };
  let request;
  globalThis.fetch = async (url, options) => {
    request = { url: String(url), options };
    return new Response(JSON.stringify({ ok: true }));
  };
  await api.login("alice", "secret");
  assert.equal(request.url, "https://example.com/commonroom/api/login");
  assert.equal(request.options.credentials, "same-origin");
  assert.equal(request.options.cache, "no-store");
  assert.deepEqual(JSON.parse(request.options.body), {
    username: "alice",
    password: "secret",
  });
  const controller = new AbortController();
  globalThis.fetch = async (url, options) => {
    request = { url: String(url), options };
    return new Response(
      JSON.stringify({ view: "@direct:bob", messages: [], revision: 0 }),
    );
  };
  await api.history("@direct:bob", controller.signal);
  assert.equal(
    request.url,
    "https://example.com/commonroom/api/history?view=%40direct%3Abob",
  );
  assert.equal(request.options.signal, controller.signal);
  assert.equal(request.options.method, "GET");
  globalThis.fetch = async () => new Response(JSON.stringify({ rooms: [] }));
  await assert.rejects(api.me(), /Invalid server response/);
  globalThis.fetch = async () =>
    new Response(JSON.stringify({ error: "Please log in." }), { status: 401 });
  await assert.rejects(api.me(), /Please log in/);
});
test("outbound frames retain the request ID and Unicode text", () => {
  let raw;
  const frame = { id: 1, room: "room", text: "@bob 你好 👋" };
  sendFrame(
    {
      send: (value) => {
        raw = value;
      },
    },
    frame,
  );
  assert.deepEqual(JSON.parse(raw), frame);
});
