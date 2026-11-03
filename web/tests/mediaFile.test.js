import assert from "node:assert/strict";
import test from "node:test";
import {
  mediaFileUrl,
  isFileLink,
  mediaDownloadUrl,
  mediaRequestUrl,
} from "../src/api/mediaFile.ts";

test("managed file identity preserves API origin and deployment while excluding unrelated URLs", () => {
  const id = "d5cd8a44-d8b7-4c63-91fe-3cc63f106178";
  for (const base of [
    "https://ui.test/",
    "https://api.test/server/",
    "http://127.0.0.1:3000/chat/",
  ]) {
    for (const kind of ["media", "avatars"]) {
      const path = `api/${kind}/${id}/file`;
      const parsed = mediaFileUrl(`${path}?playback=1`, base);
      assert.equal(parsed.base, base);
      assert.equal(parsed.kind, kind);
      assert.equal(parsed.id, id);
      assert.equal(parsed.url.href, `${base}${path}?playback=1`);
    }
    assert.equal(
      isFileLink({ url: "http://storage.test/file" }, base),
      base.startsWith("http:"),
    );
  }
  for (const url of [
    "https://api.test/api/logout",
    "https://api.test/api/media/not-a-uuid/file",
    `https://api.test/api/media/${"-".repeat(36)}/file`,
    `https://user:pass@api.test/api/media/${id}/file`,
    `https://api.test/api/media/${id}/file#fragment`,
    `ftp://api.test/api/media/${id}/file`,
    "invalid",
  ])
    assert.equal(mediaFileUrl(url), null, url);
  for (const value of [
    null,
    [],
    {},
    { url: "not a URL" },
    { url: "https://user:pass@storage.test/file" },
  ]) {
    assert.equal(isFileLink(value, "https://api.test/"), false);
  }
});

test("download navigation uses the UI cache scope and cannot become an image or frame relay", () => {
  const file =
    "https://api.test/server/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file";
  const base = "https://ui.test/chat/";
  const url = mediaDownloadUrl(`${file}?retry=1`, base);
  assert.equal(new URL(url).pathname, "/chat/media-download");
  const request = {
    url,
    method: "GET",
    mode: "navigate",
    destination: "document",
  };
  assert.equal(mediaRequestUrl(request, base)?.href, `${file}?download=true`);
  for (const change of [
    { destination: "image" },
    { destination: "iframe" },
    { destination: "" },
    { mode: "cors" },
    { method: "POST" },
  ]) {
    assert.equal(mediaRequestUrl({ ...request, ...change }, base), null);
  }
  for (const target of [
    file.replace("/media/", "/avatars/"),
    "https://api.test/api/logout",
    `https://user:pass@api.test${new URL(file).pathname}`,
  ]) {
    const route = new URL(url);
    route.searchParams.set("file", target);
    assert.equal(mediaRequestUrl({ ...request, url: route.href }, base), null);
  }
});
