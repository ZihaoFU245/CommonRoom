import { mediaResolution } from "./helpers/mediaResolve.js";
import assert from "node:assert/strict";
import test from "node:test";
import { HttpImageCache } from "../src/features/conversation/httpImageCache.ts";

const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
const href = `http://debug.test:5173/api/media/${id}/file`;

function fixture(t) {
  const calls = [];
  const urls = [];
  let status = 200;
  let release;
  let pending = Promise.resolve();
  const network = async (input, options) => {
    const url = new URL(input);
    calls.push({ url, options });
    if (url.pathname.endsWith("/resolve"))
      return mediaResolution(
        input,
        options,
        {},
        status,
        "http://debug.test:19000/object?file=" +
          encodeURIComponent(url.pathname),
      );
    await pending;
    assert.equal(options.credentials, "omit");
    return new Response("abcdef", {
      headers: { "Cache-Control": "private, no-store", "Content-Length": "6" },
    });
  };
  const cache = new HttpImageCache(network);
  t.after(() => urls.forEach((url) => URL.revokeObjectURL(url)));
  return {
    calls,
    cache,
    async load(path = href, signal = new AbortController().signal) {
      const url = await cache.load(path, signal);
      urls.push(url);
      return url;
    },
    deny() {
      status = 403;
    },
    block() {
      pending = new Promise((resolve) => {
        release = resolve;
      });
    },
    release() {
      release();
    },
    count(suffix) {
      return calls.filter(({ url }) => url.pathname.endsWith(suffix)).length;
    },
  };
}

test("HTTP media remounts reuse bytes under read leases", async (t) => {
  const f = fixture(t);
  const avatar = href.replace("/media/", "/avatars/");
  for (const path of [href, avatar, href, avatar, href]) {
    const url = await f.load(path);
    assert.ok(url.startsWith("blob:"));
    assert.equal(await (await fetch(url)).text(), "abcdef");
  }
  assert.equal(
    f.count("/resolve"),
    2,
    "each immutable file shares one read lease",
  );
  assert.equal(
    f.count("/link"),
    0,
    "resolution already supplies the signature",
  );
  assert.equal(f.count("/object"), 2, "one storage GET per immutable file");
  f.deny();
  f.cache.invalidate();
  await assert.rejects(f.load(), /403/u);
  assert.equal(
    f.count("/object"),
    2,
    "revocation cannot fall back to retained bytes",
  );
});

test("cancelled HTTP rows leave one shared admitted fill for their replacements", async (t) => {
  const f = fixture(t);
  f.block();
  const controller = new AbortController();
  const first = f.load(href, controller.signal);
  const cancelled = assert.rejects(first, { name: "AbortError" });
  while (f.count("/object") === 0)
    await new Promise((resolve) => setImmediate(resolve));
  controller.abort();
  const replacement = f.load();
  f.release();
  await cancelled;
  await replacement;
  assert.equal(
    f.count("/object"),
    1,
    "scrolling cannot restart an admitted download",
  );
});

test("HTTP image entries are bounded and deployment-specific", async (t) => {
  const f = fixture(t);
  await f.load();
  await f.load(href.replace(":5173", ":5174"));
  assert.equal(f.count("/object"), 2, "ports distinguish deployments");
  for (let index = 0; index < 128; index++) {
    const other = `${index.toString(16).padStart(8, "0")}-d8b7-4c63-91fe-3cc63f106178`;
    await f.load(href.replace(id, other));
    await new Promise((resolve) => setImmediate(resolve));
  }
  const before = f.count("/object");
  await f.load();
  assert.equal(
    f.count("/object"),
    before + 1,
    "oldest bytes are evicted after the 128-entry budget",
  );
});
