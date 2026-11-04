import assert from "node:assert/strict";
import test from "node:test";
import { randomUuid } from "../src/api/randomUuid.ts";

test("avatar upload IDs remain random v4 UUIDs when LAN HTTP lacks randomUUID", (t) => {
  t.mock.method(crypto, "randomUUID", undefined);
  const first = randomUuid();
  const second = randomUuid();
  assert.match(
    first,
    /^[a-f0-9]{8}-(?:[a-f0-9]{4}-)4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/u,
  );
  assert.notEqual(first, second);
});
