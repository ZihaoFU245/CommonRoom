import assert from "node:assert/strict";
import test from "node:test";
import { api } from "../src/api/client.ts";
import { loadInitialAccount } from "../src/api/bootstrap.ts";

test("initial and post-login snapshots honor saved opt-in, including blocked storage", async (t) => {
  const original = api.me;
  const storage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  t.after(() => {
    api.me = original;
    if (storage) Object.defineProperty(globalThis, "localStorage", storage);
    else delete globalThis.localStorage;
  });
  const account = { username: "alice", colors: { alice: "violet" } };
  const calls = [];
  api.me = async (enabled) => {
    calls.push(enabled);
    return account;
  };
  for (const saved of [null, "off", "on"]) {
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: { getItem: () => saved },
    });
    assert.equal(await loadInitialAccount(), account);
    assert.equal(calls.at(-1), saved === "on");
  }
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get() {
      throw new Error("Storage blocked");
    },
  });
  assert.equal(await loadInitialAccount(), account);
  assert.equal(calls.at(-1), false);
  assert.equal(calls.length, 4, "one bootstrap request per entry");
});
