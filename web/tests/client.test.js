import assert from "node:assert/strict";
import test from "node:test";
import { api, sendFrame, ApiError, isUnauthorized } from "../src/api/client.ts";

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
  await api.session();
  assert.equal(request.url, "https://example.com/commonroom/api/session");
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
      JSON.stringify({
        view: "@direct:bob",
        messages: [],
        revision: 0,
        has_before: false,
        has_after: false,
      }),
    );
  };
  await api.history("@direct:bob", controller.signal);
  assert.equal(
    request.url,
    "https://example.com/commonroom/api/history?view=%40direct%3Abob",
  );
  assert.equal(request.options.signal.aborted, false);
  assert.equal(request.options.method, "GET");
  for (const range of [
    { start: 0, end: 1099, limit: 100 },
    { before: 1000 },
    { after: 1099 },
    { around: 1000 },
  ]) {
    await api.history("@direct:bob", controller.signal, range);
    const url = new URL(request.url);
    for (const [key, value] of Object.entries(range))
      assert.equal(url.searchParams.get(key), String(value));
    assert.equal(request.options.signal.aborted, false);
  }
  globalThis.fetch = async () => new Response(JSON.stringify({ rooms: [] }));
  await assert.rejects(api.me(), /Invalid server response/);
  globalThis.fetch = async () =>
    new Response(JSON.stringify({ error: "Invalid or expired credential." }), {
      status: 401,
    });
  await assert.rejects(
    api.me(),
    (error) =>
      isUnauthorized(error) &&
      error.message === "Invalid or expired credential.",
  );
  globalThis.fetch = async () => new Response("Bad gateway", { status: 502 });
  await assert.rejects(
    api.me(),
    (error) => error instanceof ApiError && error.status === 502,
  );
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

test("history color data is requested only with an explicit opt-in", async (t) => {
  const originalFetch = globalThis.fetch,
    originalDocument = globalThis.document;
  t.after(() => {
    globalThis.fetch = originalFetch;
    if (originalDocument === undefined) delete globalThis.document;
    else globalThis.document = originalDocument;
  });
  globalThis.document = { baseURI: "https://example.com/commonroom/" };
  const requests = [];
  globalThis.fetch = async (_url, options) => {
    requests.push(options);
    return Response.json({
      view: "team",
      messages: [],
      revision: 1,
      has_before: false,
      has_after: false,
    });
  };
  const signal = new AbortController().signal;
  await api.history("team", signal);
  await api.history("team", signal, {}, true);
  await api.history("team", signal, {}, false);
  assert.equal(requests[0].headers?.["X-Chat-Name-Colors"], undefined);
  assert.equal(requests[1].headers["X-Chat-Name-Colors"], "1");
  assert.equal(requests[2].headers?.["X-Chat-Name-Colors"], undefined);
});
