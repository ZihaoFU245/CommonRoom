import assert from "node:assert/strict";
import test from "node:test";
import { isLibraryPage } from "../src/api/library.ts";
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
