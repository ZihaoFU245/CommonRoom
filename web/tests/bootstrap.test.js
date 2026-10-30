import assert from "node:assert/strict";
import test from "node:test";
import { api } from "../src/api/client.ts";
import { loadInitialAccount } from "../src/api/bootstrap.ts";
import {
  disconnectPreferences,
  preferenceValue,
} from "../src/features/preferences/account.ts";

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

test("login seeds only missing settings and requests its first visible snapshot using server preferences", async (t) => {
  const originalMe = api.me;
  const originalPreferences = api.preferences;
  const descriptor = Object.getOwnPropertyDescriptor(
    globalThis,
    "localStorage",
  );
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key) => (key === "commonroom-theme" ? "dark" : null),
      setItem() {},
    },
  });
  t.after(() => {
    disconnectPreferences();
    api.me = originalMe;
    api.preferences = originalPreferences;
    if (descriptor)
      Object.defineProperty(globalThis, "localStorage", descriptor);
    else delete globalThis.localStorage;
  });
  const calls = [];
  const account = {
    username: "alice",
    account_access: { id: "alice-id" },
    preferences: { name_colors: "true", theme: "light" },
    sync: { instance: "server", revision: 1 },
  };
  api.me = async (colors) => {
    calls.push(colors);
    return account;
  };
  api.preferences = async (values, initialize) => {
    assert.equal(initialize, true);
    assert.equal(
      values.theme,
      "dark",
      "send local settings to seed unset keys",
    );
    assert.equal(values.name_colors, "false");
    return {
      values: { ...values, ...account.preferences },
      sync: account.sync,
    };
  };
  assert.equal(await loadInitialAccount(), account);
  assert.deepEqual(calls, [false, true]);
  assert.equal(preferenceValue("theme", ""), "light");
});
