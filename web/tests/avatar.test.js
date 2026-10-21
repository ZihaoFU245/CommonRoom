import test from "node:test";
import assert from "node:assert/strict";
import {
  avatarSource,
  cropSquare,
} from "../src/features/preferences/avatar.ts";

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
    ),
    "https://gravatar.com/avatar/hash?s=128",
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
    assert.equal(avatarSource(source, "https://chat.example/"), undefined);
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
