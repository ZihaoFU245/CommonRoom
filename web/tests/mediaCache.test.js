import assert from "node:assert/strict";
import test from "node:test";
import {
  MediaCache,
  mediaCachePath,
} from "../src/features/conversation/mediaCache.ts";
const base = "https://chat.test/chat/";
const path = `${base}api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file`;
function fixture(filePath = path) {
  const saved = new Map();
  const cache = {
    match: async (key) => saved.get(String(key.url ?? key))?.clone(),
    keys: async () => [...saved.keys()].map((key) => new Request(key)),
    delete: async (key) => saved.delete(String(key.url ?? key)),
    put: async (key, response) => {
      const body = await response.arrayBuffer();
      saved.set(
        String(key.url ?? key),
        new Response(body, { headers: response.headers }),
      );
    },
  };
  const storage = { open: async () => cache };
  const calls = [];
  let status = 200;
  let contentType = "image/png";
  const network = async (url, options) => {
    calls.push({ url: String(url), options });
    if (String(url).endsWith("/info"))
      return Response.json(
        { size: 6, content_type: contentType, filename: "test'file.html" },
        { status },
      );
    if (options.headers.Range)
      return new Response("abc", {
        status: 206,
        headers: { "Content-Length": "3" },
      });
    return new Response("abcdef");
  };
  const work = [];
  const load = (instance, query = "", range) => {
    const request = new Request(filePath + query, {
      headers: range ? { Range: range } : {},
    });
    return instance.respond(request, new URL(request.url), (promise) =>
      work.push(promise),
    );
  };
  return {
    storage,
    network,
    calls,
    work,
    load,
    saved,
    deny: () => {
      status = 403;
    },
    html: () => {
      contentType = "text/html";
    },
  };
}
test("immutable bytes survive reloads, queries share bytes and authorization stays live", async () => {
  const f = fixture();
  assert.equal(
    await (await f.load(new MediaCache(f.storage, f.network))).text(),
    "abcdef",
  );
  await Promise.all(f.work);
  const reload = new MediaCache(f.storage, f.network);
  assert.equal(await (await f.load(reload, "?playback=2")).text(), "abcdef");
  const download = await f.load(reload, "?download=true");
  assert.match(download.headers.get("Content-Disposition"), /^attachment;/);
  assert.equal(f.calls.filter((call) => !call.url.endsWith("/info")).length, 1);
  assert.equal(f.calls.filter((call) => call.url.endsWith("/info")).length, 3);
  f.deny();
  assert.equal((await f.load(reload)).status, 403);
  assert.equal(f.calls.filter((call) => !call.url.endsWith("/info")).length, 1);
});
test("cached full objects satisfy native ranges and never render HTML inline", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  await (await f.load(instance)).text();
  await Promise.all(f.work);
  for (const [range, expected] of [
    ["bytes=1-3", "bcd"],
    ["bytes=-2", "ef"],
    ["bytes=4-", "ef"],
  ]) {
    const response = await f.load(instance, "", range);
    assert.equal(response.status, 206);
    assert.equal(await response.text(), expected);
  }
  assert.equal((await f.load(instance, "", "bytes=99-")).status, 416);
  assert.equal(f.calls.filter((call) => !call.url.endsWith("/info")).length, 1);
  f.html();
  const html = await f.load(instance);
  assert.match(html.headers.get("Content-Disposition"), /^attachment;/);
  assert.equal(html.headers.get("X-Content-Type-Options"), "nosniff");
});
test("partial playback responses preserve legacy CORS support without caching fragments", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  const first = await f.load(instance, "?playback=0", "bytes=0-2");
  assert.equal(first.status, 206);
  assert.equal(first.headers.get("Content-Range"), "bytes 0-2/6");
  assert.equal(await first.text(), "abc");
  await Promise.all(f.work);
  assert.equal(f.saved.size, 0);
  const next = await f.load(
    new MediaCache(f.storage, f.network),
    "?playback=1",
    "bytes=0-2",
  );
  assert.equal(next.status, 206);
  assert.equal(await next.text(), "abc");
  assert.equal(f.calls.filter((call) => !call.url.endsWith("/info")).length, 2);
});
test("storage failures preserve downloads and worker matching respects the deployment scope", async () => {
  const f = fixture();
  const instance = new MediaCache(
    {
      open: async () => {
        throw new Error("disabled");
      },
    },
    f.network,
  );
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  assert.ok(mediaCachePath(new Request(path), base));
  for (const url of [
    path.replace("chat.test", "evil.test"),
    path.replace("/chat/", "/other/"),
    path.replace("/file", "/info"),
    `${base}api/logout`,
  ])
    assert.equal(mediaCachePath(new Request(url), base), null);
  assert.equal(
    mediaCachePath(new Request(path, { method: "POST" }), base),
    null,
  );
});

test("default browser fetch is invoked without a class-instance receiver", async (t) => {
  const f = fixture();
  const original = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = original;
  });
  globalThis.fetch = function (...args) {
    assert.ok(this === undefined || this === globalThis);
    return f.network(...args);
  };
  assert.equal(
    await (await f.load(new MediaCache(f.storage))).text(),
    "abcdef",
  );
  await Promise.all(f.work);
});

test("persistent cache evicts old entries rather than growing without bounds", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  let first;
  for (let index = 0; index < 129; index++) {
    const url = new URL(
      path.replace(
        /d5cd8a44-d8b7-4c63-91fe-3cc63f106178/u,
        `d5cd8a44-d8b7-4c63-91fe-${String(index).padStart(12, "0")}`,
      ),
    );
    first ??= url.href;
    await (
      await instance.respond(new Request(url), url, (work) => f.work.push(work))
    ).arrayBuffer();
    await Promise.all(f.work);
  }
  assert.equal(f.saved.size, 128);
  assert.equal(f.saved.has(first), false);
});

test("cache hits protect recently viewed objects from eviction", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  const urls = Array.from(
    { length: 129 },
    (_, index) =>
      new URL(
        path.replace(
          /d5cd8a44-d8b7-4c63-91fe-3cc63f106178/u,
          `d5cd8a44-d8b7-4c63-91fe-${String(index).padStart(12, "0")}`,
        ),
      ),
  );
  async function load(url) {
    await (
      await instance.respond(new Request(url), url, (work) => f.work.push(work))
    ).arrayBuffer();
    await Promise.all(f.work);
  }
  for (const url of urls.slice(0, 128)) await load(url);
  await load(urls[0]);
  await load(urls[128]);
  assert.equal(f.saved.size, 128);
  assert.equal(f.saved.has(urls[0].href), true);
  assert.equal(f.saved.has(urls[1].href), false);
});

test("actual network Content-Length wins over info and cached ranges use that length", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, async (url, options) =>
    String(url).endsWith("/info")
      ? f.network(url, options)
      : new Response("abcd", { headers: { "Content-Length": "4" } }),
  );
  const full = await f.load(instance);
  assert.equal(full.headers.get("Content-Length"), "4");
  assert.equal(await full.text(), "abcd");
  await Promise.all(f.work);
  const range = await f.load(instance, "", "bytes=-2");
  assert.equal(range.headers.get("Content-Range"), "bytes 2-3/4");
  assert.equal(await range.text(), "cd");
});

test("managed avatar bytes reuse the cache after reload but removal and logout deny reuse", async () => {
  const avatar = path.replace("api/media/", "api/avatars/");
  const f = fixture(avatar);
  assert.equal(mediaCachePath(new Request(avatar), base)?.href, avatar);
  assert.equal(
    mediaCachePath(new Request(avatar), "https://chat.test/other/"),
    null,
  );
  const first = new MediaCache(f.storage, f.network);
  assert.equal(await (await f.load(first)).text(), "abcdef");
  await Promise.all(f.work);
  const reload = new MediaCache(f.storage, f.network);
  assert.equal(await (await f.load(reload)).text(), "abcdef");
  assert.equal(f.calls.filter((call) => !call.url.endsWith("/info")).length, 1);
  f.deny();
  assert.equal((await f.load(reload)).status, 403);
  assert.equal(f.calls.filter((call) => !call.url.endsWith("/info")).length, 1);
});
