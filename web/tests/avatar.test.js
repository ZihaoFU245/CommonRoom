import test from "node:test";
import assert from "node:assert/strict";
import {
  avatarSource,
  applyAvatar,
  cropSquare,
  encodeAvatar,
  MAX_AVATAR_BYTES,
  isCompletedAvatar,
} from "../src/features/preferences/avatar.ts";

test("avatar encoding bounds bytes, requests WebP and shrinks PNG fallback", async () => {
  const attempts = [];
  const output = {
    width: 0,
    height: 0,
    getContext: () => ({ drawImage() {} }),
    toBlob(callback, type, quality) {
      attempts.push([this.width, this.height, type, quality]);
      callback(
        new Blob([new Uint8Array(this.width * this.height * 4)], {
          type: "image/png",
        }),
      );
    },
  };
  const source = { ownerDocument: { createElement: () => output } };
  const blob = await encodeAvatar(source);
  assert.equal(MAX_AVATAR_BYTES, 65536);
  assert.equal(blob.type, "image/png");
  assert.equal(blob.size, MAX_AVATAR_BYTES);
  assert.deepEqual(attempts, [
    [256, 256, "image/webp", 0.8],
    [128, 128, "image/webp", 0.8],
  ]);
  output.toBlob = (callback) =>
    callback(new Blob(["small"], { type: "image/webp" }));
  assert.equal((await encodeAvatar(source)).type, "image/webp");
  output.toBlob = (callback) => callback(new Blob([new Uint8Array(65537)]));
  await assert.rejects(encodeAvatar(source), /exceeds 64 KiB/u);
  output.toBlob = (callback) => callback(null);
  await assert.rejects(encodeAvatar(source), /Unable to encode/u);
});

test("avatar sources preserve base paths and reject unsafe URLs", () => {
  const id = "12345678-1234-1234-1234-123456789abc";
  assert.equal(
    avatarSource(`/api/avatars/${id}/file`, "https://chat.example/team/"),
    `https://chat.example/team/api/avatars/${id}/file`,
  );
  assert.equal(
    avatarSource(
      "https://gravatar.com/avatar/hash?s=128",
      "https://chat.example/",
      true,
    ),
    "https://gravatar.com/avatar/hash?s=128",
  );
  assert.equal(
    avatarSource("https://gravatar.com/avatar/hash", "https://chat.example/"),
    undefined,
  );
  for (const source of [
    undefined,
    "",
    "http://example.com/a",
    "javascript:alert(1)",
    "data:image/png;base64,AA",
    "https://user:pass@example.com/a",
    "/api/media/id/file",
  ])
    assert.equal(
      avatarSource(source, "https://chat.example/", true),
      undefined,
    );
});
test("square crop centers landscape and portrait and respects zoom and edge positions", () => {
  assert.deepEqual(cropSquare(800, 400, 1, 50, 50), {
    x: 200,
    y: 0,
    size: 400,
  });
  assert.deepEqual(cropSquare(400, 800, 1, 50, 50), {
    x: 0,
    y: 200,
    size: 400,
  });
  assert.deepEqual(cropSquare(800, 400, 2, 100, 100), {
    x: 600,
    y: 200,
    size: 200,
  });
  assert.deepEqual(cropSquare(800, 400, 2, -10, 110), {
    x: 0,
    y: 200,
    size: 200,
  });
  for (const args of [
    [0, 400, 1, 50, 50],
    [400, NaN, 1, 50, 50],
    [400, 400, 0, 50, 50],
  ])
    assert.throws(() => cropSquare(...args));
});

test("avatar completion requires a matching canonical media ID and path", () => {
  const media_id = "12345678-1234-1234-1234-123456789abc";
  const valid = { ok: true, media_id, path: `/api/avatars/${media_id}/file` };
  assert.equal(isCompletedAvatar(valid), true);
  for (const invalid of [
    null,
    {},
    { ...valid, ok: false },
    { ...valid, path: "https://tracker.test/image" },
    { ...valid, media_id: "forged" },
    { ...valid, path: "/api/media/other/file" },
  ])
    assert.equal(isCompletedAvatar(invalid), false);
});

test("profile updates replace or remove one avatar without replacing history or other profiles", () => {
  const rooms = [{ messages: [{ text: "preserved" }] }];
  const initial = {
    username: "alice",
    rooms,
    avatars: { alice: "old", bob: "other" },
  };
  const updated = applyAvatar(initial, {
    kind: "avatar",
    username: "alice",
    url: "new",
  });
  assert.deepEqual(updated.avatars, { alice: "new", bob: "other" });
  assert.equal(updated.rooms, rooms);
  assert.equal(initial.avatars.alice, "old");
  const removed = applyAvatar(updated, {
    kind: "avatar",
    username: "alice",
    url: null,
  });
  assert.deepEqual(removed.avatars, { bob: "other" });
  const special = applyAvatar(removed, {
    kind: "avatar",
    username: "__proto__",
    url: "safe-own-property",
  });
  assert.equal(Object.getPrototypeOf(special.avatars), Object.prototype);
  assert.equal(Object.hasOwn(special.avatars, "__proto__"), true);
});
