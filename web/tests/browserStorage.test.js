import assert from "node:assert/strict";
import test from "node:test";
import {
  readStorage,
  readStorageJson,
  writeStorage,
} from "../src/api/browserStorage.ts";

test("browser storage handles policy and quota failures at its boundary", (t) => {
  const descriptor = Object.getOwnPropertyDescriptor(
    globalThis,
    "localStorage",
  );
  t.after(() => {
    if (descriptor)
      Object.defineProperty(globalThis, "localStorage", descriptor);
    else delete globalThis.localStorage;
  });
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get() {
      throw new DOMException("Blocked", "SecurityError");
    },
  });
  assert.equal(readStorage("key"), null);
  assert.equal(writeStorage("key", "value"), false);
  assert.equal(readStorageJson("key"), null);
  assert.equal(
    writeStorage("key", "value", {
      setItem() {
        throw new DOMException("Full", "QuotaExceededError");
      },
    }),
    false,
  );
});

test("invalid stored JSON uses defaults but unexpected failures propagate unchanged", () => {
  assert.equal(readStorageJson("key", { getItem: () => "broken" }), null);
  assert.deepEqual(readStorageJson("key", { getItem: () => '{"value":1}' }), {
    value: 1,
  });
  const failure = new Error("Programming error");
  const broken = {
    getItem() {
      throw failure;
    },
    setItem() {
      throw failure;
    },
  };
  assert.throws(
    () => readStorage("key", broken),
    (error) => error === failure,
  );
  assert.throws(
    () => readStorageJson("key", broken),
    (error) => error === failure,
  );
  assert.throws(
    () => writeStorage("key", "value", broken),
    (error) => error === failure,
  );
});
