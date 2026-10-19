import assert from "node:assert/strict";
import test from "node:test";
import {
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
  const pending = uploadFile(ticket, file, signal.signal, (value) =>
    progress.push(value),
  );
  assert.equal(xhr.withCredentials, false);
  assert.equal(xhr.url, ticket.upload.url);
  assert.deepEqual(xhr.headers, { "Content-Type": "application/octet-stream" });
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
