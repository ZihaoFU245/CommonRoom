import assert from "node:assert/strict";
import test from "node:test";
import {
  isLibraryPage,
  isLibrarySections,
  sharedLibrarySections,
} from "../src/api/library.ts";
import { parseFrame } from "../src/api/protocol.ts";
import { api } from "../src/api/client.ts";

const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
const page = {
  items: [
    {
      media_id: id,
      filename: "wave.gif",
      content_type: "image/gif",
      size: 8,
      available: true,
      owned: false,
      saved: true,
      owner_count: 2,
    },
  ],
  next_cursor: null,
  usage: 0,
  quota: 1000,
  cleanup_bytes: 0,
  cleanup_objects: 0,
  oldest_cleanup: null,
  uploads_paused: false,
};
test("library responses validate bounded lists and explicit ownership independently of membership", () => {
  assert.ok(isLibraryPage(page));
  assert.ok(
    isLibraryPage({
      ...page,
      items: [{ ...page.items[0], owned: true, saved: false }],
    }),
  );
  for (const patch of [
    { items: Array(101).fill(page.items[0]) },
    { quota: -1 },
    { usage: 1.5 },
    { uploads_paused: "yes" },
    { items: [{ ...page.items[0], media_id: "../other" }] },
    { items: [{ ...page.items[0], owner_count: -1 }] },
  ]) {
    assert.equal(isLibraryPage({ ...page, ...patch }), false);
  }
  assert.deepEqual(parseFrame('{"kind":"media"}'), { kind: "media" });
  const ids = ["d5cd8a44-d8b7-4c63-91fe-3cc63f106178"];
  assert.deepEqual(parseFrame(JSON.stringify({ kind: "media", ids })), {
    kind: "media",
    ids,
  });
  assert.equal(parseFrame('{"kind":"media","ids":["../foreign"]}'), null);
});

test("Recent and Collections share one authenticated batch read; pagination stays independent", async (t) => {
  const previous = globalThis.document;
  globalThis.document = { baseURI: "https://chat.example.com/commonroom/" };
  t.after(() => {
    globalThis.document = previous;
  });
  const requests = [];
  t.mock.method(globalThis, "fetch", async (url, options) => {
    requests.push({ url: String(url), options });
    return Response.json(
      options.method !== "GET"
        ? { ok: true }
        : String(url).includes("sections=true")
          ? { recent: page, library: page }
          : page,
    );
  });
  const controller = new AbortController();
  const results = await Promise.all([
    api.library("recent", "", controller.signal, "alice"),
    api.library("library", "", controller.signal, "alice"),
  ]);
  assert.equal(requests.length, 1);
  assert.match(requests[0].url, /library\?sections=true$/u);
  assert.equal(requests[0].options.credentials, "include");
  assert.deepEqual(results, [page, page]);
  await api.library("library", "50", controller.signal, "alice");
  assert.match(requests[1].url, /after=50$/u);
  assert.equal(
    isLibrarySections({ library: page, recent: { ...page, usage: -1 } }),
    false,
  );
  await api.libraryMove(id, { before_id: id });
  assert.deepEqual(JSON.parse(requests[2].options.body), {
    action: "move",
    before_id: id,
  });
});

test("batched library reads release cancelled consumers and isolate accounts", async () => {
  let complete;
  let calls = 0;
  let sharedSignal;
  const load = (signal) => {
    calls++;
    sharedSignal = signal;
    return new Promise((resolve) => {
      complete = resolve;
    });
  };
  const a = new AbortController(),
    b = new AbortController();
  const first = sharedLibrarySections("alice", a.signal, load);
  const second = sharedLibrarySections("alice", b.signal, load);
  await Promise.resolve();
  assert.equal(calls, 1);
  a.abort();
  await assert.rejects(first, { name: "AbortError" });
  assert.equal(sharedSignal.aborted, false);
  complete({ library: page, recent: page });
  assert.deepEqual(await second, { library: page, recent: page });
  const c = new AbortController();
  const cancelled = sharedLibrarySections("alice", c.signal, load);
  await Promise.resolve();
  c.abort();
  await assert.rejects(cancelled, { name: "AbortError" });
  assert.equal(sharedSignal.aborted, true);
  const different = sharedLibrarySections(
    "bob",
    new AbortController().signal,
    async () => ({ library: page, recent: page }),
  );
  assert.deepEqual(await different, { library: page, recent: page });
});
test("library requests preserve base paths, cookies and explicit final-release confirmation", async (t) => {
  const document = globalThis.document;
  globalThis.document = { baseURI: "https://chat.example.com/commonroom/" };
  t.after(() => {
    globalThis.document = document;
  });
  const requests = [];
  t.mock.method(globalThis, "fetch", async (url, options) => {
    requests.push({ url: String(url), options });
    return Response.json(options.method === "GET" ? page : { ok: true });
  });
  await api.library("owned", "cursor", new AbortController().signal);
  await api.libraryChange(id, "release", undefined, true);
  assert.equal(
    requests[0].url,
    "https://chat.example.com/commonroom/api/library?owned=true&recent=false&after=cursor",
  );
  assert.equal(
    requests[1].url,
    `https://chat.example.com/commonroom/api/library/${id}`,
  );
  assert.deepEqual(JSON.parse(requests[1].options.body), {
    action: "release",
    source_conversation: null,
    confirm_last: true,
  });
  assert.equal(requests[1].options.credentials, "include");
  assert.equal(requests[1].options.cache, "default");
});
