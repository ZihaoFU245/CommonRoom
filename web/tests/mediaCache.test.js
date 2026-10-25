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
  let puts = 0;
  const reads = { keys: 0, matches: 0 };
  const cache = {
    match: async (key) => {
      reads.matches++;
      return saved.get(String(key.url ?? key))?.clone();
    },
    keys: async () => {
      reads.keys++;
      return [...saved.keys()].map((key) => new Request(key));
    },
    delete: async (key) => saved.delete(String(key.url ?? key)),
    put: async (key, response) => {
      puts++;
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
  let expired = false;
  let contentType = "image/png";
  const network = async (url, options) => {
    calls.push({ url: String(url), options });
    if (String(url).endsWith("/info"))
      return Response.json(
        {
          size: 6,
          content_type: contentType,
          filename: "test'file.html",
          expired,
        },
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
    reads,
    puts: () => puts,
    expire: () => {
      expired = true;
    },
    deny: () => {
      status = 403;
    },
    html: () => {
      contentType = "text/html";
    },
  };
}
test("repeated full and range hits rate-limit recency writes while checking live access", async (t) => {
  let now = 100_000;
  t.mock.method(Date, "now", () => now);
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  await (await f.load(instance)).text();
  await Promise.all(f.work);
  await (await f.load(instance)).text();
  await Promise.all(f.work);
  const writes = f.puts();
  for (const range of [undefined, "bytes=1-3", "bytes=-2"]) {
    await (await f.load(instance, "", range)).text();
    await Promise.all(f.work);
  }
  assert.equal(f.puts(), writes);
  assert.equal(f.calls.filter((call) => call.url.endsWith("/info")).length, 5);
  now += 60_000;
  await (await f.load(instance)).text();
  await Promise.all(f.work);
  assert.equal(f.puts(), writes + 1);
  f.deny();
  assert.equal((await f.load(instance)).status, 403);
  assert.equal(f.puts(), writes + 1);
});
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

test("cache accounting reuses response sizes and enforces byte limits after restart", async () => {
  const f = fixture();
  const bytes = 150 * 1024 * 1024;
  const network = (url, options) =>
    String(url).endsWith("/info")
      ? f.network(url, options)
      : new Response("abcdef", {
          headers: { "Content-Length": String(bytes) },
        });
  const first = new MediaCache(f.storage, network);
  async function load(instance, index) {
    const url = new URL(
      path.replace("06178/file", `${String(index).padStart(5, "0")}/file`),
    );
    await (
      await instance.respond(new Request(url), url, (p) => f.work.push(p))
    ).text();
    await Promise.all(f.work);
    return url.href;
  }
  const a = await load(first, 1);
  const b = await load(first, 2);
  assert.equal(f.saved.has(a), false);
  assert.equal(f.saved.has(b), true);
  assert.equal(f.reads.keys, 2);
  assert.equal(f.reads.matches, 2, "inserts must not rescan cached responses");
  const restarted = new MediaCache(f.storage, network);
  const c = await load(restarted, 3);
  assert.equal(f.reads.keys, 3);
  assert.equal(f.saved.has(b), false);
  assert.equal(f.saved.has(c), true);
  assert.equal(f.saved.size, 1);
});

test("failed maintenance rebuilds accounting and external eviction stays bounded", async (t) => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  await (await f.load(instance)).text();
  await Promise.all(f.work);
  const cache = await f.storage.open();
  const put = cache.put;
  let fail = true;
  t.mock.method(cache, "put", (...args) => {
    if (fail) {
      fail = false;
      throw new Error("quota");
    }
    return put(...args);
  });
  // A failed hit touch deletes the entry before put fails; the next miss must
  // recover its accounting and download normally, without stale byte totals.
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  await Promise.all(f.work);
  assert.equal(f.saved.size, 0);
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  await Promise.all(f.work);
  assert.equal(f.reads.keys, 2);
  f.saved.clear();
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  await Promise.all(f.work);
  assert.equal(f.saved.size, 1);
});

test("overlapping worker lifetimes cannot leave cache accounting undercounted", async () => {
  const f = fixture();
  const first = new MediaCache(f.storage, f.network);
  const replacement = new MediaCache(f.storage, f.network);
  async function load(worker, index) {
    const url = new URL(
      path.replace(
        /d5cd8a44-d8b7-4c63-91fe-3cc63f106178/u,
        `d5cd8a44-d8b7-4c63-91fe-${String(index).padStart(12, "0")}`,
      ),
    );
    await (
      await worker.respond(new Request(url), url, (p) => f.work.push(p))
    ).text();
    await Promise.all(f.work);
  }
  for (let index = 0; index < 128; index++) await load(first, index);
  await load(replacement, 128);
  await load(first, 129);
  assert.equal(f.saved.size, 128);
});

test("concurrent cold reads coalesce one media fill", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  const responses = await Promise.all(
    Array.from({ length: 16 }, () => f.load(instance)),
  );
  await Promise.all(responses.map((response) => response.text()));
  await Promise.all(f.work);
  const downloads = f.calls.filter(
    (call) => !call.url.endsWith("/info"),
  ).length;

  assert.equal(
    downloads,
    1,
    "concurrent identical misses should share one fill",
  );
});

test("cold-fill admission bounds concurrent cache copies and releases slots", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  const cache = await f.storage.open();
  const put = cache.put;
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  cache.put = async (...args) => {
    await gate;
    return put(...args);
  };
  const urls = Array.from(
    { length: 16 },
    (_, index) =>
      new URL(
        path.replace("06178/file", `${String(index).padStart(5, "0")}/file`),
      ),
  );
  const responses = await Promise.all(
    urls.map((url) =>
      instance.respond(new Request(url), url, (work) => f.work.push(work)),
    ),
  );
  await Promise.all(responses.map((response) => response.text()));
  assert.equal(
    f.puts(),
    0,
    "network downloads remain usable while cache writes wait",
  );
  release();
  await Promise.all(f.work);
  assert.equal(f.puts(), 4, "only four response copies can await cache writes");
  const url = new URL(path.replace("06178/file", "99999/file"));
  await (
    await instance.respond(new Request(url), url, (work) => f.work.push(work))
  ).text();
  await Promise.all(f.work);
  assert.equal(f.puts(), 5, "completed fills return their admission slots");
});

test("cold-fill byte reservations bound large copies independently of entry count", async () => {
  const f = fixture();
  const size = 150 * 1024 * 1024;
  const instance = new MediaCache(f.storage, async (url, options) =>
    String(url).endsWith("/info")
      ? Response.json({
          size,
          content_type: "image/png",
          filename: "large.png",
        })
      : new Response("abcdef", { headers: { "Content-Length": String(size) } }),
  );
  const cache = await f.storage.open();
  const put = cache.put;
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  cache.put = async (...args) => {
    await gate;
    return put(...args);
  };
  const urls = [
    new URL(path),
    new URL(path.replace("06178/file", "00000/file")),
  ];
  const responses = await Promise.all(
    urls.map((url) =>
      instance.respond(new Request(url), url, (work) => f.work.push(work)),
    ),
  );
  await Promise.all(responses.map((response) => response.text()));
  release();
  await Promise.all(f.work);
  assert.equal(
    f.puts(),
    1,
    "two 150 MiB copies must not enter a 256 MiB fill budget",
  );
});

test("a cancelled fill waiter exits without cancelling another caller's download", async () => {
  const f = fixture();
  let release, started;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const downloading = new Promise((resolve) => {
    started = resolve;
  });
  const instance = new MediaCache(f.storage, async (url, options) => {
    if (!String(url).endsWith("/info")) {
      started();
      await gate;
    }
    return f.network(url, options);
  });
  const first = f.load(instance);
  await downloading;
  const cancellation = new AbortController();
  const waiter = instance.respond(
    new Request(path, { signal: cancellation.signal }),
    new URL(path),
    (work) => f.work.push(work),
  );
  await new Promise((resolve) => setImmediate(resolve));
  cancellation.abort(new DOMException("cancelled", "AbortError"));
  await assert.rejects(waiter, { name: "AbortError" });
  release();
  assert.equal(await (await first).text(), "abcdef");
  await Promise.all(f.work);
  assert.equal(f.calls.filter((call) => !call.url.endsWith("/info")).length, 1);
});

test("failed fills release waiters and every caller still checks authorization", async () => {
  const f = fixture();
  let release, started;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  const downloading = new Promise((resolve) => {
    started = resolve;
  });
  let firstDownload = true;
  const instance = new MediaCache(f.storage, async (url, options) => {
    if (!String(url).endsWith("/info") && firstDownload) {
      firstDownload = false;
      started();
      await gate;
      throw new Error("storage offline");
    }
    return f.network(url, options);
  });
  const first = assert.rejects(f.load(instance), /storage offline/);
  await downloading;
  const waiter = f.load(instance);
  await new Promise((resolve) => setImmediate(resolve));
  f.deny();
  assert.equal(
    (await f.load(instance)).status,
    403,
    "a denied caller cannot join an authorized fill",
  );
  release();
  await first;
  assert.equal(await (await waiter).text(), "abcdef");
  await Promise.all(f.work);
  assert.equal(
    (await f.load(instance)).status,
    403,
    "persisted bytes do not bypass authorization",
  );
});

test("expired metadata never serves cached bytes or fetches storage", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  assert.equal((await f.load(instance)).status, 200);
  await Promise.all(f.work);
  f.expire();
  for (const range of [undefined, "bytes=0-2"]) {
    const response = await f.load(instance, "", range);
    assert.equal(response.status, 410);
    assert.equal(await response.text(), "");
  }
  assert.equal(f.calls.filter((call) => !call.url.endsWith("/info")).length, 1);
});
