import assert from "node:assert/strict";
import test from "node:test";
import {
  documentSource,
  previewLimit,
  attachmentKind,
  attachmentLink,
  isCompletedMedia,
  isUploadTicket,
  mediaMarkdown,
  uploadFile,
} from "../src/api/media.ts";
import { api } from "../src/api/client.ts";

const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
const media = {
  media_id: id,
  path: `/commonroom/api/media/${id}/file`,
  filename: "screen[shot].png",
  content_type: "image/png",
  size: 123,
};
const ticket = {
  media_id: id,
  upload: {
    method: "PUT",
    url: "https://storage.example.com/bucket/uploads/test?signature=secret",
    headers: { "Content-Type": "application/octet-stream" },
    expires_in: 300,
  },
};

test("media responses reject unsafe destinations and preserve base paths", () => {
  assert.ok(isCompletedMedia(media));
  assert.ok(isUploadTicket(ticket));
  for (const path of [
    "//evil.test/file",
    "https://evil.test/file",
    "/api/media/../../logout",
    "/api/media/not-a-uuid/file",
  ])
    assert.equal(isCompletedMedia({ ...media, path }), false);
  for (const url of [
    "javascript:alert(1)",
    "file:///tmp/file",
    "https://user:password@storage.example.com/file",
  ])
    assert.equal(
      isUploadTicket({ ...ticket, upload: { ...ticket.upload, url } }),
      false,
    );
  assert.equal(
    mediaMarkdown(media, "https://chat.example.com/commonroom/"),
    `![screen_shot_.png](https://chat.example.com/commonroom/api/media/${id}/file)`,
  );
  assert.ok(
    mediaMarkdown(
      { ...media, content_type: "application/octet-stream" },
      "https://chat.example.com/",
    ).startsWith("["),
  );
  assert.equal(isCompletedMedia({ ...media, size: 0 }), false);
});

test("media API sends cookies only to CommonRoom and validates completion", async (t) => {
  const original = { fetch: globalThis.fetch, document: globalThis.document };
  t.after(() => Object.assign(globalThis, original));
  globalThis.document = { baseURI: "https://chat.example.com/commonroom/" };
  const calls = [];
  globalThis.fetch = async (url, options) => {
    calls.push({ url: String(url), options });
    return new Response(
      JSON.stringify(String(url).endsWith("/complete") ? media : ticket),
    );
  };
  const signal = new AbortController().signal;
  await api.allocateMedia("room-id", "photo.png", 123, signal);
  await api.completeMedia(id, signal);
  assert.equal(
    calls[0].url,
    "https://chat.example.com/commonroom/api/media/uploads",
  );
  assert.deepEqual(JSON.parse(calls[0].options.body), {
    conversation_id: "room-id",
    filename: "photo.png",
    size: 123,
    hints: {},
  });
  assert.equal(calls[1].options.credentials, "same-origin");
  assert.equal(calls[1].options.cache, "no-store");
  globalThis.fetch = async () =>
    new Response(JSON.stringify({ ...media, path: "https://evil.test" }));
  await assert.rejects(
    api.completeMedia(id, signal),
    /Invalid server response/,
  );
});

test("direct upload omits chat cookies, reports progress and aborts", async (t) => {
  const original = globalThis.XMLHttpRequest;
  t.after(() => {
    globalThis.XMLHttpRequest = original;
  });
  let xhr;
  globalThis.XMLHttpRequest = class {
    upload = {};
    headers = {};
    constructor() {
      xhr = this;
    }
    open(method, url) {
      this.method = method;
      this.url = url;
    }
    setRequestHeader(name, value) {
      this.headers[name] = value;
    }
    send(file) {
      this.file = file;
    }
    abort() {
      this.onabort();
    }
  };
  const signal = new AbortController();
  const progress = [];
  const file = new File(["hello"], "file.txt");
  const hintedTicket = {
    ...ticket,
    upload: {
      ...ticket.upload,
      headers: {
        ...ticket.upload.headers,
        "x-amz-meta-client-label": "file hint",
      },
    },
  };
  const pending = uploadFile(hintedTicket, file, signal.signal, (value) =>
    progress.push(value),
  );
  assert.equal(xhr.withCredentials, false);
  assert.equal(xhr.url, ticket.upload.url);
  assert.deepEqual(xhr.headers, {
    "Content-Type": "application/octet-stream",
    "x-amz-meta-client-label": "file hint",
  });
  xhr.upload.onprogress({ lengthComputable: true, loaded: 3, total: 5 });
  assert.deepEqual(progress, [60]);
  signal.abort();
  await assert.rejects(pending, { name: "AbortError" });
  const success = uploadFile(
    ticket,
    file,
    new AbortController().signal,
    () => {},
  );
  xhr.status = 200;
  xhr.onload();
  await success;
});

test("native attachment hints preserve Markdown and cannot target external origins", () => {
  const base = "https://chat.example.com/commonroom/";
  for (const [mime, kind] of [
    ["audio/mpeg", "audio"],
    ["video/mp4", "video"],
    ["text/html", "html"],
    ["image/svg+xml", "svg"],
  ]) {
    assert.equal(attachmentKind(mime), kind);
    assert.equal(
      mediaMarkdown({ ...media, content_type: mime }, base),
      `[screen_shot_.png](https://chat.example.com/commonroom/api/media/${id}/file "media:${kind}")`,
    );
    assert.deepEqual(
      attachmentLink(new URL(media.path, base).href, `media:${kind}`, base),
      { id, kind, path: media.path },
    );
  }
  assert.ok(
    mediaMarkdown({ ...media, content_type: "image/avif" }, base).startsWith(
      "!",
    ),
  );
  for (const path of [
    `https://evil.test/commonroom/api/media/${id}/file`,
    `https://user:secret@chat.example.com${media.path}`,
    `/api/media/${id}/file`,
    `${media.path}?x=1`,
    `${media.path}#x`,
    `/commonroom/api/media/not-a-uuid/file`,
  ])
    assert.equal(attachmentLink(path, "media:video", base), null);
  assert.equal(attachmentLink(media.path, "media:script", base), null);
});

test("document previews bound streaming bytes, reject invalid UTF-8, and release streams", async (t) => {
  const original = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = original;
  });
  const controller = new AbortController();
  let cancelled = false;
  globalThis.fetch = async () =>
    new Response(
      new ReadableStream({
        pull(c) {
          c.enqueue(new Uint8Array(previewLimit + 1));
        },
        cancel() {
          cancelled = true;
        },
      }),
    );
  await assert.rejects(
    documentSource("/api/media/file", controller.signal),
    /1 MiB/,
  );
  assert.equal(cancelled, true);
  globalThis.fetch = async () => new Response(new Uint8Array([0xff]));
  await assert.rejects(
    documentSource("/api/media/file", controller.signal),
    /encoded data/,
  );
  globalThis.fetch = async (_url, options) => {
    assert.equal(options.credentials, "same-origin");
    assert.equal(options.cache, "no-store");
    return new Response("<html>preview</html>");
  };
  assert.equal(
    await documentSource("/api/media/file", controller.signal),
    "<html>preview</html>",
  );
});

test("attachment paths share validation, malformed origins fail without throwing, and progress is bounded", async (t) => {
  assert.equal(
    mediaMarkdown(
      { ...media, path: "https://evil.test/file" },
      "https://chat.example.com/",
    ),
    null,
  );
  assert.equal(mediaMarkdown(media, "not a base URL"), null);
  assert.equal(
    isCompletedMedia({
      ...media,
      media_id: "00000000-0000-0000-0000-000000000000",
    }),
    false,
  );
  assert.equal(
    isUploadTicket({ ...ticket, upload: { ...ticket.upload, expires_in: -1 } }),
    false,
  );
  const original = globalThis.XMLHttpRequest;
  const originalPerformance = globalThis.performance;
  globalThis.performance = { now: () => 0 };
  let xhr;
  t.after(() => {
    globalThis.XMLHttpRequest = original;
    globalThis.performance = originalPerformance;
  });
  globalThis.XMLHttpRequest = class {
    upload = {};
    constructor() {
      xhr = this;
    }
    open() {}
    setRequestHeader() {}
    send() {}
  };
  const values = [];
  const pending = uploadFile(
    ticket,
    new File(["test"], "test"),
    new AbortController().signal,
    (value) => values.push(value),
  );
  for (let loaded = 1; loaded <= 100; loaded++)
    xhr.upload.onprogress({ lengthComputable: true, loaded, total: 100 });
  assert.deepEqual(values, [1, 100]);
  assert.equal(xhr.timeout, 15 * 60 * 1000);
  xhr.status = 200;
  xhr.onload();
  await pending;
});

test("upload hint headers are bounded and cannot inject cookies or server headers", () => {
  const withHeaders = (headers) => ({
    ...ticket,
    upload: {
      ...ticket.upload,
      headers: { ...ticket.upload.headers, ...headers },
    },
  });
  assert.ok(
    isUploadTicket(
      withHeaders({
        "x-amz-meta-client-filename": "%E4%BD%A0.png",
        "x-amz-meta-client-content-type": "text/html",
      }),
    ),
  );
  for (const headers of [
    { Cookie: "session=secret" },
    { Authorization: "Bearer secret" },
    { "x-amz-meta-trusted-uploader": "forged" },
    { "x-amz-meta-client-label": "bad\r\nheader: injected" },
    { "x-amz-meta-client-label": "x".repeat(257) },
    { "x-amz-meta-client-label": 42 },
  ])
    assert.equal(isUploadTicket(withHeaders(headers)), false);
});
