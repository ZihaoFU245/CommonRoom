import { mediaResolution } from "./helpers/mediaResolve.js";
import assert from "node:assert/strict";
import test from "node:test";
import { MediaCache } from "../src/features/conversation/mediaCache.ts";
const base = "https://chat.test/chat/";
const path = `${base}api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file`;
function fixture(filePath = path) {
  const saved = new Map();
  let puts = 0;
  const reads = { keys: 0, matches: 0, recencyMatches: 0 };
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
  const recency = new Map();
  let metadataPuts = 0;
  const metadataCache = {
    match: async (key) => {
      reads.recencyMatches++;
      return recency.get(String(key.url ?? key))?.clone();
    },
    keys: async () => [...recency.keys()].map((key) => new Request(key)),
    delete: async (key) => recency.delete(String(key.url ?? key)),
    put: async (key, response) => {
      metadataPuts++;
      recency.set(String(key.url ?? key), response.clone());
    },
  };
  const storage = {
    open: async (name) =>
      name === "commonroom-media-recency" ? metadataCache : cache,
  };
  const calls = [];
  let status = 200;
  let expired = false;
  let contentType = "image/png";
  const network = async (url, options) => {
    calls.push({ url: String(url), options });
    if (String(url).endsWith("/resolve"))
      return mediaResolution(
        url,
        options,
        { content_type: contentType, filename: "test'file.html", expired },
        status,
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
    metadataPuts: () => metadataPuts,
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
test("repeated full and range hits persist small recency records without rewriting bytes", async (t) => {
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
  assert.equal(
    f.calls.filter((call) => call.url.endsWith("/resolve")).length,
    1,
  );
  now += 60_000;
  await (await f.load(instance)).text();
  await Promise.all(f.work);
  assert.equal(f.puts(), writes, "recency must not rewrite attachment bytes");
  f.deny();
  instance.invalidate();
  assert.equal((await f.load(instance)).status, 403);
  assert.equal(f.puts(), writes, "recency must not rewrite attachment bytes");
});
for (const kind of ["media", "avatars"]) {
  test(`${kind} bytes survive reloads, share query variants and recheck authorization`, async () => {
    const f = fixture(path.replace("api/media/", `api/${kind}/`));
    assert.equal(
      await (await f.load(new MediaCache(f.storage, f.network))).text(),
      "abcdef",
    );
    await Promise.all(f.work);
    const reload = new MediaCache(f.storage, f.network);
    assert.equal(await (await f.load(reload, "?playback=2")).text(), "abcdef");
    const download = await f.load(reload, "?download=true");
    assert.match(download.headers.get("Content-Disposition"), /^attachment;/);
    assert.equal(
      f.calls.filter((call) => !call.url.endsWith("/resolve")).length,
      1,
    );
    assert.equal(
      f.calls.filter((call) => call.url.endsWith("/resolve")).length,
      2,
    );
    f.deny();
    reload.invalidate();
    assert.equal((await f.load(reload)).status, 403);
    assert.equal(
      f.calls.filter((call) => !call.url.endsWith("/resolve")).length,
      1,
    );
  });
}
test("incompatible cached records are deleted and refetched without migrations", async () => {
  const cases = [
    { status: 206 },
    { length: null },
    { length: "invalid" },
    { length: "0" },
    { length: "1.5" },
    { length: "268435457" },
    { type: null },
    { type: "text/html" },
    { extra: { "Content-Range": "bytes 0-2/6" } },
    { extra: { "Content-Encoding": "gzip" } },
    { empty: true },
  ];
  for (const invalid of cases) {
    const f = fixture();
    const headers = new Headers({
      "Content-Length": "6",
      "Content-Type": "image/png",
      ...invalid.extra,
    });
    for (const [field, header] of [
      ["length", "Content-Length"],
      ["type", "Content-Type"],
    ]) {
      if (!(field in invalid)) continue;
      if (invalid[field] === null) headers.delete(header);
      else headers.set(header, invalid[field]);
    }
    f.saved.set(
      path,
      new Response(invalid.empty ? null : "broken", {
        status: invalid.status ?? 200,
        headers,
      }),
    );
    const instance = new MediaCache(f.storage, async (url, options) => {
      if (!String(url).endsWith("/resolve"))
        assert.equal(f.saved.has(path), false, JSON.stringify(invalid));
      return f.network(url, options);
    });
    assert.equal(await (await f.load(instance)).text(), "abcdef");
    await Promise.all(f.work);
    assert.equal(
      await (
        await f.load(new MediaCache(f.storage, f.network), "", "bytes=1-3")
      ).text(),
      "bcd",
    );
    await Promise.all(f.work);
    assert.equal(
      f.calls.filter((call) => !call.url.endsWith("/resolve")).length,
      1,
    );
    assert.equal(f.saved.size, 1);
  }
});

test("range reads discard incompatible cached bytes before fetching their range", async () => {
  const f = fixture();
  f.saved.set(path, new Response("broken"));
  const instance = new MediaCache(f.storage, f.network);
  const range = await f.load(instance, "", "bytes=1-3");
  assert.equal(range.status, 206);
  assert.equal(await range.text(), "abc");
  assert.equal(f.saved.size, 0);
  assert.equal(
    f.puts(),
    0,
    "a partial read cannot replace a full cached object",
  );
});

test("cache accounting discards incompatible entries instead of counting them as zero bytes", async () => {
  const f = fixture();
  const invalid = path.replace("06178/file", "99999/file");
  f.saved.set(invalid, new Response("broken"));
  assert.equal(
    await (await f.load(new MediaCache(f.storage, f.network))).text(),
    "abcdef",
  );
  await Promise.all(f.work);
  assert.equal(f.saved.has(invalid), false);
  assert.equal(f.saved.size, 1);
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
  assert.equal(
    f.calls.filter((call) => !call.url.endsWith("/resolve")).length,
    1,
  );
  f.html();
  instance.invalidate();
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
  assert.equal(
    f.calls.filter((call) => !call.url.endsWith("/resolve")).length,
    2,
  );
});
test("unavailable storage cannot silently switch to uncached downloads", async () => {
  const f = fixture();
  const instance = new MediaCache(
    {
      open: async () => {
        throw new Error("disabled");
      },
    },
    f.network,
  );
  await assert.rejects(f.load(instance), /disabled/u);
  assert.ok(
    f.calls.every((call) => call.url.endsWith("/resolve")),
    "only live authorization ran; no storage download bypassed the cache",
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

test("cache entry limits evict the oldest object and warm hits protect recent objects", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  const urls = Array.from(
    { length: 130 },
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
  await load(urls[128]);
  assert.equal(f.saved.size, 128);
  assert.equal(f.saved.has(urls[0].href), false);
  await load(urls[1]);
  await load(urls[129]);
  assert.equal(f.saved.size, 128);
  assert.equal(f.saved.has(urls[1].href), true);
  assert.equal(f.saved.has(urls[2].href), false);
});

test("bulk byte eviction reads recency once per entry instead of once per removed object", async () => {
  const f = fixture();
  const large = new URL(path.replace("06178/file", "99999/file"));
  const instance = new MediaCache(f.storage, async (url, options) =>
    String(url).endsWith("/resolve")
      ? f.network(url, options)
      : new Response("abcdef", {
          headers: {
            "Content-Length": String(
              (String(url) === large.href ? 200 : 1) * 1024 * 1024,
            ),
          },
        }),
  );
  async function load(url) {
    await (
      await instance.respond(new Request(url), url, (work) => f.work.push(work))
    ).text();
    await Promise.all(f.work);
  }
  const urls = Array.from(
    { length: 128 },
    (_, index) =>
      new URL(
        path.replace("06178/file", `${String(index).padStart(5, "0")}/file`),
      ),
  );
  for (const url of urls) await load(url);
  await load(urls[0]);
  const before = f.reads.recencyMatches;
  await load(large);
  assert.equal(
    f.reads.recencyMatches - before,
    128,
    "one bounded ranking pass handles all 72 evictions",
  );
  assert.equal(f.saved.size, 57);
  assert.ok(f.saved.has(urls[0].href), "recently viewed bytes stay protected");
  assert.ok(f.saved.has(large.href));
});

test("invalid recency records cannot protect stale bytes from eviction", async () => {
  const f = fixture();
  const recency = await f.storage.open("commonroom-media-recency");
  for (let index = 0; index < 128; index++) {
    const key = path.replace(
      "06178/file",
      `${String(index).padStart(5, "0")}/file`,
    );
    f.saved.set(
      key,
      new Response("abcdef", {
        headers: { "Content-Length": "6", "Content-Type": "image/png" },
      }),
    );
    await recency.put(
      key,
      new Response(null, {
        headers: { "X-Accessed-At": index === 0 ? "Infinity" : "1000" },
      }),
    );
  }
  const instance = new MediaCache(f.storage, f.network);
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  await Promise.all(f.work);
  const invalid = path.replace("06178/file", "00000/file");
  assert.equal(f.saved.has(invalid), false);
  assert.equal(await recency.match(invalid), undefined);
  assert.equal(f.saved.size, 128);
});

test("slow recency storage cannot build an unbounded write queue during repeated warm reads", async (t) => {
  let now = 100000;
  t.mock.method(Date, "now", () => now);
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  const urls = Array.from(
    { length: 128 },
    (_, index) =>
      new URL(
        path.replace("06178/file", `${String(index).padStart(5, "0")}/file`),
      ),
  );
  const load = async (url) =>
    (
      await instance.respond(new Request(url), url, (work) => f.work.push(work))
    ).text();
  for (const url of urls) {
    await load(url);
    await Promise.all(f.work);
  }
  const bytes = f.puts(),
    tasks = f.work.length,
    writes = f.metadataPuts();
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  t.after(() => release());
  const recency = await f.storage.open("commonroom-media-recency");
  const put = recency.put;
  t.mock.method(recency, "put", async (...args) => {
    await gate;
    return put(...args);
  });
  for (let index = 0; index < 512; index++) {
    now += 60001;
    assert.equal(await load(urls[index % urls.length]), "abcdef");
  }
  assert.equal(
    f.work.length - tasks,
    128,
    "queued writes are bounded and duplicate touches coalesce",
  );
  assert.equal(f.puts(), bytes, "warm reads never rewrite file bodies");
  release();
  await Promise.all(f.work);
  assert.equal(f.metadataPuts() - writes, 128);
});

test("actual network Content-Length wins over info and cached ranges use that length", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, async (url, options) =>
    String(url).endsWith("/resolve")
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

test("cache accounting reuses response sizes and enforces byte limits after restart", async () => {
  const f = fixture();
  const bytes = 150 * 1024 * 1024;
  const network = (url, options) =>
    String(url).endsWith("/resolve")
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
  assert.ok(
    f.reads.matches <= 6,
    "inserts do bounded lookups, never scan all cached responses",
  );
  const restarted = new MediaCache(f.storage, network);
  const c = await load(restarted, 3);
  assert.equal(f.reads.keys, 3);
  assert.equal(f.saved.has(b), false);
  assert.equal(f.saved.has(c), true);
  assert.equal(f.saved.size, 1);
});

test("failed recency writes preserve bytes and external eviction stays bounded", async (t) => {
  const f = fixture();
  let now = 100000;
  t.mock.method(Date, "now", () => now);
  const instance = new MediaCache(f.storage, f.network);
  await (await f.load(instance)).text();
  await Promise.all(f.work);
  const cache = await f.storage.open("commonroom-media-recency");
  t.mock.method(cache, "put", () => {
    throw new DOMException("quota", "QuotaExceededError");
  });
  now += 60000;
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  await Promise.all(f.work);
  assert.equal(
    f.saved.size,
    1,
    "metadata failures must not delete immutable bytes",
  );
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  f.saved.clear();
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  await Promise.all(f.work);
  assert.equal(f.saved.size, 1);
});

test("unexpected cache writer failures surface without poisoning later fills", async (t) => {
  const f = fixture();
  const cache = await f.storage.open("commonroom-media");
  const put = cache.put;
  const failure = new Error("Broken cache writer");
  let fail = true;
  t.mock.method(cache, "put", (...args) => {
    if (fail) {
      fail = false;
      throw failure;
    }
    return put(...args);
  });
  const instance = new MediaCache(f.storage, f.network);
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  const results = await Promise.allSettled(f.work);
  assert.ok(
    results.some(
      (result) => result.status === "rejected" && result.reason === failure,
    ),
  );
  f.work.length = 0;
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

test("origin-wide maintenance serializes overlapping workers before accounting and eviction", async (t) => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  let queue = Promise.resolve();
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      locks: {
        request(name, callback) {
          assert.equal(name, "commonroom-media");
          const work = queue.then(callback);
          queue = work.catch(() => {});
          return work;
        },
      },
    },
  });
  t.after(() => {
    if (original) Object.defineProperty(globalThis, "navigator", original);
    else delete globalThis.navigator;
  });
  const f = fixture();
  const network = (url, options) =>
    String(url).endsWith("/resolve")
      ? f.network(url, options)
      : new Response("abcdef", {
          headers: { "Content-Length": String(150 * 1024 * 1024) },
        });
  let release;
  const gate = new Promise((resolve) => {
    release = resolve;
  });
  t.after(() => release());
  const cache = await f.storage.open();
  const put = cache.put;
  t.mock.method(cache, "put", async (...args) => {
    await gate;
    return put(...args);
  });
  const workers = [
    new MediaCache(f.storage, network),
    new MediaCache(f.storage, network),
  ];
  await Promise.all(
    workers.map(async (worker, index) => {
      const url = new URL(
        path.replace("06178/file", `${String(index).padStart(5, "0")}/file`),
      );
      return (
        await worker.respond(new Request(url), url, (work) => f.work.push(work))
      ).text();
    }),
  );
  release();
  await Promise.all(f.work);
  assert.equal(
    f.saved.size,
    1,
    "two simultaneous 150 MiB fills cannot exceed the shared 256 MiB disk budget",
  );
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
    (call) => !call.url.endsWith("/resolve"),
  ).length;

  assert.equal(
    downloads,
    1,
    "concurrent identical misses should share one fill",
  );
});

test("remote managed files share stable bytes across remounts, reloads, queries and ranges", async () => {
  const f = fixture();
  const calls = [];
  let denied = false;
  const network = async (address, options) => {
    const url = String(address);
    calls.push({ url, options });
    if (url.endsWith("/resolve")) {
      assert.equal(options.credentials, "include");
      return mediaResolution(
        address,
        options,
        { filename: "remote.png" },
        denied ? 403 : 200,
        "https://store.test/object?signature=one",
      );
    }
    assert.equal(options.credentials, "omit");
    assert.equal(url, "https://store.test/object?signature=one");
    return new Response("abcdef", {
      headers: { "Cache-Control": "private, no-store" },
    });
  };
  const load = async (instance, target, range) => {
    return instance.respond(
      new Request(target, { headers: range ? { Range: range } : {} }),
      target,
      (work) => f.work.push(work),
      true,
    );
  };
  for (const kind of ["media", "avatars"]) {
    const url = new URL(
      path.replace("chat.test", "remote.test").replace("/media/", `/${kind}/`),
    );
    const first = new MediaCache(f.storage, network);
    const responses = await Promise.all(
      Array.from({ length: 12 }, () => load(first, url)),
    );
    assert.deepEqual(
      await Promise.all(responses.map((r) => r.text())),
      Array(12).fill("abcdef"),
    );
    await Promise.all(f.work);
    const reload = new MediaCache(f.storage, network);
    const queried = new URL(`${url}?playback=2`);
    assert.equal(await (await load(reload, queried)).text(), "abcdef");
    const range = await load(reload, queried, "bytes=1-3");
    assert.equal(range.status, 206);
    assert.equal(await range.text(), "bcd");
    const download = await load(reload, new URL(`${url}?download=true`));
    assert.match(download.headers.get("Content-Disposition"), /^attachment;/);
    assert.equal(await download.text(), "abcdef");
  }
  assert.equal(calls.filter((c) => c.url.includes("store.test")).length, 2);
  assert.equal(calls.filter((c) => c.url.endsWith("/resolve")).length, 4);
  // Identical IDs under different deployments cannot share bytes.
  await (
    await load(
      new MediaCache(f.storage, network),
      new URL(path.replace("chat.test", "another.test")),
    )
  ).text();
  await Promise.all(f.work);
  assert.equal(calls.filter((c) => c.url.includes("store.test")).length, 3);
  denied = true;
  assert.equal(
    (
      await load(
        new MediaCache(f.storage, network),
        new URL(path.replace("chat.test", "remote.test")),
      )
    ).status,
    403,
  );
  assert.equal(calls.filter((c) => c.url.includes("store.test")).length, 3);
});

test(
  "an admitted whole-file fill survives row cancellation without blocking ranges",
  { timeout: 2000 },
  async (t) => {
    const f = fixture();
    let started, release, downloadSignal;
    const downloading = new Promise((resolve) => {
      started = resolve;
    });
    const gate = new Promise((resolve) => {
      release = resolve;
    });
    t.after(() => release());
    const instance = new MediaCache(f.storage, async (url, options) => {
      if (!String(url).endsWith("/resolve") && !options.headers?.Range) {
        downloadSignal = options.signal;
        started();
        await gate;
        options.signal.throwIfAborted();
      }
      return f.network(url, options);
    });
    const controller = new AbortController();
    const first = instance.respond(
      new Request(path, { signal: controller.signal }),
      new URL(path),
      (work) => f.work.push(work),
    );
    await downloading;
    controller.abort();
    assert.equal(downloadSignal.aborted, false);
    const remount = f.load(instance);
    const seeking = f.load(instance, "", "bytes=1-3");
    assert.equal(await (await seeking).text(), "abc");
    assert.equal(f.saved.size, 0, "seeking finishes before the full download");
    release();
    assert.equal(await (await first).text(), "abcdef");
    assert.equal(await (await remount).text(), "abcdef");
    await Promise.all(f.work);
    assert.equal(f.calls.filter((c) => !c.url.endsWith("/resolve")).length, 2);
    assert.equal(f.calls.filter((c) => c.url.endsWith("/resolve")).length, 1);
  },
);

test("a slow active download can finish beyond the background-fill deadline", async (t) => {
  const f = fixture();
  let stream, downloadSignal;
  const instance = new MediaCache(f.storage, async (url, options) => {
    if (String(url).endsWith("/resolve")) return f.network(url, options);
    downloadSignal = options.signal;
    return new Response(
      new ReadableStream({
        start(controller) {
          stream = controller;
          controller.enqueue(new TextEncoder().encode("abc"));
          options.signal.addEventListener(
            "abort",
            () => controller.error(options.signal.reason),
            { once: true },
          );
        },
      }),
    );
  });
  const response = await f.load(instance);
  const body = response.text().catch((error) => error);
  t.mock.timers.enable({ apis: ["setTimeout"] });
  t.mock.timers.tick(120_001);
  assert.equal(downloadSignal.aborted, false);
  stream.enqueue(new TextEncoder().encode("def"));
  stream.close();
  assert.equal(await body, "abcdef");
  await Promise.all(f.work);
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  await Promise.all(f.work);
});

test("a cancelled consumer leaves only a bounded background fill", async (t) => {
  const f = fixture();
  let downloadSignal;
  let first = true;
  const instance = new MediaCache(f.storage, async (url, options) => {
    if (String(url).endsWith("/resolve") || !first)
      return f.network(url, options);
    first = false;
    downloadSignal = options.signal;
    return new Response(
      new ReadableStream({
        start(controller) {
          options.signal.addEventListener(
            "abort",
            () => controller.error(options.signal.reason),
            { once: true },
          );
        },
      }),
    );
  });
  const controller = new AbortController();
  const response = await instance.respond(
    new Request(path, { signal: controller.signal }),
    new URL(path),
    (work) => f.work.push(work),
  );
  const body = response.text().catch((error) => error);
  t.mock.timers.enable({ apis: ["setTimeout"] });
  t.mock.timers.tick(120_001);
  assert.equal(downloadSignal.aborted, false);
  controller.abort();
  t.mock.timers.tick(119_999);
  assert.equal(downloadSignal.aborted, false);
  t.mock.timers.tick(1);
  assert.equal(downloadSignal.aborted, true);
  assert.equal((await body).name, "TimeoutError");
  await Promise.all(f.work);
  assert.equal(f.saved.size, 0, "incomplete bytes are not cached");
  t.mock.timers.reset();
  assert.equal(await (await f.load(instance)).text(), "abcdef");
  await Promise.all(f.work);
  assert.equal(f.saved.size, 1, "the expired fill releases its reservation");
});

for (const { label, size, count, admitted } of [
  { label: "entry count", size: 6, count: 16, admitted: 4 },
  {
    label: "byte reservations",
    size: 150 * 1024 * 1024,
    count: 2,
    admitted: 1,
  },
]) {
  test(`cold-fill ${label} bounds cache copies and releases reservations`, async () => {
    const f = fixture();
    const instance = new MediaCache(f.storage, async (url, options) =>
      String(url).endsWith("/resolve")
        ? mediaResolution(url, options, { size })
        : new Response("abcdef", {
            headers: { "Content-Length": String(size) },
          }),
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
    const urls = Array.from(
      { length: count },
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
    assert.deepEqual(
      await Promise.all(responses.map((response) => response.text())),
      urls.map(() => "abcdef"),
    );
    assert.equal(
      f.puts(),
      0,
      "downloads remain usable while cache writes wait",
    );
    release();
    await Promise.all(f.work);
    assert.equal(f.puts(), admitted, label);
    const url = new URL(path.replace("06178/file", "99999/file"));
    await (
      await instance.respond(new Request(url), url, (work) => f.work.push(work))
    ).text();
    await Promise.all(f.work);
    assert.equal(
      f.puts(),
      admitted + 1,
      "completed fills return their reservations",
    );
  });
}

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
    if (!String(url).endsWith("/resolve")) {
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
  assert.equal(
    f.calls.filter((call) => !call.url.endsWith("/resolve")).length,
    1,
  );
});

test("failed fills release waiters; revoked leases deny reads", async () => {
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
    if (!String(url).endsWith("/resolve") && firstDownload) {
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
  release();
  await first;
  assert.equal(await (await waiter).text(), "abcdef");
  await Promise.all(f.work);
  f.deny();
  instance.invalidate();
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
  instance.invalidate();
  for (const range of [undefined, "bytes=0-2"]) {
    const response = await f.load(instance, "", range);
    assert.equal(response.status, 410);
    assert.equal(await response.text(), "");
  }
  assert.equal(
    f.calls.filter((call) => !call.url.endsWith("/resolve")).length,
    1,
  );
});

test("recency metadata stays bounded when the browser independently evicts bodies", async () => {
  const f = fixture();
  const instance = new MediaCache(f.storage, f.network);
  for (let index = 0; index < 140; index++) {
    f.saved.clear();
    const url = new URL(
      path.replace("06178/file", `${String(index).padStart(5, "0")}/file`),
    );
    await (
      await instance.respond(new Request(url), url, (work) => f.work.push(work))
    ).text();
    await Promise.all(f.work);
  }
  const recency = await f.storage.open("commonroom-media-recency");
  assert.ok((await recency.keys()).length <= 128);
  assert.equal(f.saved.size, 1);
});

test("metadata leases require neither Cache Storage nor unused signed links", async () => {
  let calls = 0;
  const cache = new MediaCache(
    {
      open: () => {
        throw new Error("Storage unavailable");
      },
    },
    async (url, options) => {
      calls++;
      assert.equal(JSON.parse(options.body).items[0].link, false);
      return mediaResolution(url, options);
    },
  );
  await cache.resolve(path);
  await cache.resolve(path);
  assert.equal(calls, 1);
});
