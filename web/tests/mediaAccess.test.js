import assert from "node:assert/strict";
import test from "node:test";
import {
  MediaAccess,
  MediaAccessError,
} from "../src/features/conversation/mediaAccess.ts";
import { mediaResolution } from "./helpers/mediaResolve.js";
const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
const href = `https://chat.test/chat/api/media/${id}/file`;

function fixture() {
  const calls = [];
  let status = 200;
  const access = new MediaAccess(async (url, options) => {
    calls.push({
      url: String(url),
      options,
      items: JSON.parse(options.body).items,
    });
    return mediaResolution(url, options, {}, status);
  });
  return {
    access,
    calls,
    deny: () => {
      status = 403;
    },
  };
}

test("media reads share bounded batches", async () => {
  const { access, calls } = fixture();
  const files = Array.from({ length: 40 }, (_, index) =>
    href.replace(
      id,
      `${index.toString(16).padStart(8, "0")}-d8b7-4c63-91fe-3cc63f106178`,
    ),
  );
  const values = await Promise.all(
    [...files, ...files].map((file) => access.load(file, true)),
  );
  assert.equal(values.length, 80);
  assert.deepEqual(
    calls.map((call) => call.items.length),
    [32, 8],
  );
  for (const { options } of calls) {
    assert.equal(options.credentials, "include");
    assert.equal(
      options.cache,
      "default",
      "reuse credentialed CORS preflights",
    );
    assert.ok(Buffer.byteLength(options.body) <= 4096);
  }
  assert.equal(
    calls.every((call) => call.url.endsWith("/api/media/resolve")),
    true,
  );
});

test("valid leases skip network; expired leases recheck access", async (t) => {
  let now = 100000;
  t.mock.method(Date, "now", () => now);
  const { access, calls, deny } = fixture();
  const first = await access.load(href, true);
  assert.equal(first.expiresAt, now + 900000);
  assert.ok(first.renewAt >= now + 675000 && first.renewAt <= now + 765000);
  now += 600000;
  for (let index = 0; index < 20; index++) await access.load(href);
  assert.equal(calls.length, 1);
  deny();
  now = first.expiresAt;
  await assert.rejects(
    access.load(href),
    (error) => error instanceof MediaAccessError && error.status === 403,
  );
  assert.equal(calls.length, 2);
});

test("signed links isolate expiry and disposition", async (t) => {
  let now = 100000;
  t.mock.method(Date, "now", () => now);
  const { access, calls } = fixture();
  await access.load(href, true);
  now += 61000;
  await access.load(href);
  assert.equal(
    calls.length,
    1,
    "local bytes still have an authorization lease",
  );
  await access.load(href, true);
  assert.equal(calls.length, 2, "a cold byte miss obtains a fresh signed URL");
  await access.load(href, true, false, true);
  assert.equal(calls.length, 3);
  assert.equal(calls.at(-1).items[0].download, true);
  await access.load(href);
  await access.load(href, true);
  await access.load(href, true, false, true);
  assert.equal(
    calls.length,
    3,
    "inline and download links share read authority",
  );
});

test("queued display and byte reads share metadata and signing", async () => {
  const { access, calls } = fixture();
  const [one, two] = await Promise.all([
    access.load(href),
    access.load(href, true),
  ]);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].items[0].link, true);
  assert.equal(one.url, two.url);
});

test("targeted invalidation isolates assets and deployments", async () => {
  const { access, calls } = fixture();
  const other = href.replace(id, "00000000-d8b7-4c63-91fe-3cc63f106178");
  await Promise.all([access.load(href), access.load(other)]);
  access.clear([id]);
  await access.load(other);
  assert.equal(calls.length, 1);
  await access.load(href);
  await access.load(href.replace("chat.test", "other.test"));
  await access.load(href.replace("/chat/", "/another/"));
  assert.equal(calls.length, 4);
});

test("invalidation rejects queued and late read authority", async () => {
  let finish;
  let started;
  const entered = new Promise((resolve) => {
    started = resolve;
  });
  const access = new MediaAccess(
    (url, options) =>
      new Promise((resolve) => {
        finish = () => resolve(mediaResolution(url, options));
        started();
      }),
  );
  const read = access.load(href);
  const rejected = assert.rejects(read, { name: "AbortError" });
  await entered;
  access.clear();
  finish();
  await rejected;
  const replacement = access.load(href);
  const again = assert.rejects(replacement, { name: "AbortError" });
  access.clear();
  await again;
});

test("leases respect media expiry and reject invalid metadata", async (t) => {
  t.mock.method(Date, "now", () => 100000);
  const access = new MediaAccess((url, options) =>
    Promise.resolve(mediaResolution(url, options, { expires: 105 })),
  );
  assert.equal((await access.load(href)).expiresAt, 105000);
  const invalid = new MediaAccess((url, options) =>
    Promise.resolve(
      mediaResolution(url, options, { path: `/foreign/api/media/${id}/file` }),
    ),
  );
  await assert.rejects(invalid.load(href), /Invalid media authorization/u);
  const oversized = new MediaAccess(async (url, options) => {
    const response = await mediaResolution(url, options).json();
    response.items[0].lease_seconds = 86401;
    return Response.json(response);
  });
  await assert.rejects(oversized.load(href), /Invalid media authorization/u);
});

test("session rejection clears leases and retains 401 responses", async () => {
  const access = new MediaAccess(
    async () => new Response(null, { status: 401 }),
  );
  await assert.rejects(
    access.load(href),
    (error) => error instanceof MediaAccessError && error.status === 401,
  );
});

test("render bursts share a bounded lane per deployment", async () => {
  let finish, entered;
  const started = new Promise((resolve) => {
    entered = resolve;
  });
  const gate = new Promise((resolve) => {
    finish = resolve;
  });
  let calls = 0;
  const access = new MediaAccess(async (url, options) => {
    calls++;
    if (calls === 1) {
      entered();
      await gate;
    }
    return mediaResolution(url, options);
  });
  const first = access.load(href);
  await started;
  const next = access.load(
    href.replace(id, "00000000-d8b7-4c63-91fe-3cc63f106178"),
  );
  await new Promise((resolve) => setTimeout(resolve, 15));
  assert.equal(calls, 1, "slow reads cannot exhaust the account's HTTP budget");
  finish();
  await Promise.all([first, next]);
  assert.equal(calls, 2);
});

test("targeted invalidation preserves unrelated reads in the same in-flight batch", async () => {
  let finish, entered;
  const started = new Promise((resolve) => {
    entered = resolve;
  });
  const access = new MediaAccess(
    (url, options) =>
      new Promise((resolve) => {
        finish = () => resolve(mediaResolution(url, options));
        entered();
      }),
  );
  const other = href.replace(id, "00000000-d8b7-4c63-91fe-3cc63f106178");
  const affected = access.load(href);
  const rejected = assert.rejects(affected, { name: "AbortError" });
  const unrelated = access.load(other);
  await started;
  access.clear([]);
  access.clear([id]);
  finish();
  await rejected;
  assert.equal(
    (await unrelated).info.media_id,
    new URL(other).pathname.split("/").at(-2),
  );
});

test("authority conflicts retry once and preserve successful batch results", async () => {
  for (const conflicts of [1, 2]) {
    let calls = 0;
    const access = new MediaAccess(async (url, options) => {
      calls++;
      return calls <= conflicts
        ? new Response(null, { status: 409 })
        : mediaResolution(url, options);
    });
    if (conflicts === 1)
      assert.equal((await access.load(href)).info.media_id, id);
    else
      await assert.rejects(access.load(href), (error) => error.status === 409);
    assert.equal(calls, 2);
  }
});

test("a one-second signed link can be reused without extending its lifetime", async (t) => {
  let now = 100000,
    calls = 0;
  t.mock.method(Date, "now", () => now);
  const access = new MediaAccess(async (url, options) => {
    calls++;
    const packet = await mediaResolution(url, options).json();
    packet.items[0].link.expires_in = 1;
    return Response.json(packet);
  });
  await access.load(href, true);
  await access.load(href, true);
  assert.equal(calls, 1);
  now += 1000;
  await access.load(href, true);
  assert.equal(calls, 2);
});

test("zero-duration leases and links fail at the response boundary", async () => {
  for (const field of ["lease", "link"]) {
    const access = new MediaAccess(async (url, options) => {
      const packet = await mediaResolution(url, options).json();
      if (field === "lease") packet.items[0].lease_seconds = 0;
      else packet.items[0].link.expires_in = 0;
      return Response.json(packet);
    });
    await assert.rejects(access.load(href, true), /Invalid media/u);
  }
});

test("malformed later items reject the whole packet without installing early leases", async () => {
  let calls = 0;
  const other = href.replace(id, "00000000-d8b7-4c63-91fe-3cc63f106178");
  const access = new MediaAccess(async (url, options) => {
    calls++;
    const packet = await mediaResolution(url, options).json();
    if (calls === 1) packet.items[1].lease_seconds = 0;
    return Response.json(packet);
  });
  const results = await Promise.allSettled([
    access.load(href),
    access.load(other),
  ]);
  assert.ok(results.every((result) => result.status === "rejected"));
  await Promise.all([access.load(href), access.load(other)]);
  assert.equal(
    calls,
    2,
    "neither a lease nor pending ownership survived rejection",
  );
});

test("conflict retries share one deadline and omit invalidated items", async (t) => {
  const timers = t.mock.method(globalThis, "setTimeout");
  const other = href.replace(id, "00000000-d8b7-4c63-91fe-3cc63f106178");
  let calls = 0,
    signal;
  const access = new MediaAccess(async (url, options) => {
    calls++;
    if (calls === 1) {
      signal = options.signal;
      access.clear([id]);
      return new Response(null, { status: 409 });
    }
    assert.equal(options.signal, signal);
    assert.deepEqual(
      JSON.parse(options.body).items.map((item) => item.id),
      [other.split("/").at(-2)],
    );
    return mediaResolution(url, options);
  });
  await Promise.all([
    assert.rejects(access.load(href), { name: "AbortError" }),
    access.load(other),
  ]);
  assert.equal(calls, 2);
  assert.equal(
    timers.mock.calls.filter((call) => call.arguments[1] === 15000).length,
    1,
  );
});
