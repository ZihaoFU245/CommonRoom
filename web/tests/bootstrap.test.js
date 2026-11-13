import assert from "node:assert/strict";
import test from "node:test";
import { api } from "../src/api/client.ts";
import { loadInitialAccount } from "../src/api/bootstrap.ts";
import {
  connectPreferences,
  disconnectPreferences,
  preferenceValue,
} from "../src/features/preferences/account.ts";

test("initial and post-login snapshots request colors by default and honor saved choices", async (t) => {
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
    assert.equal(calls.at(-1), saved !== "off");
  }
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get() {
      throw new DOMException("Storage blocked", "SecurityError");
    },
  });
  assert.equal(await loadInitialAccount(), account);
  assert.equal(calls.at(-1), true);
  assert.equal(calls.length, 4, "one bootstrap request per entry");
});

test("bootstrap seeds settings without a second snapshot request", async (t) => {
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
    preferences: { name_colors: "false", theme: "light" },
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
    assert.equal(values.markdown, "true");
    assert.equal(values.name_colors, "true");
    assert.equal(values.message_bars, "true");
    return {
      values: { ...values, ...account.preferences },
      sync: account.sync,
    };
  };
  assert.equal(await loadInitialAccount(), account);
  assert.deepEqual(calls, [true]);
  assert.equal(preferenceValue("name_colors", ""), "false");
  assert.equal(preferenceValue("theme", ""), "light");
});

for (const complete of [true, false]) {
  test(`bootstrap reuses snapshot (complete=${complete})`, async (t) => {
    const originalMe = api.me,
      originalPreferences = api.preferences;
    t.after(() => {
      api.me = originalMe;
      api.preferences = originalPreferences;
      disconnectPreferences();
    });
    let values;
    const sync = { instance: "server", revision: 1 };
    await connectPreferences("seed", async (local) => {
      values = local;
      return { values: local, sync };
    });
    const account = {
      account_access: { id: "alice-id" },
      preferences: complete ? values : {},
      sync,
    };
    let reads = 0,
      writes = 0;
    api.me = async () => {
      reads++;
      return account;
    };
    api.preferences = async (local) => {
      writes++;
      return { values: local, sync };
    };
    assert.equal(await loadInitialAccount(), account);
    assert.equal(reads, 1);
    assert.equal(writes, complete ? 0 : 1);
  });
}

test("first snapshot resolves while preference initialization is pending", async (t) => {
  const originalMe = api.me,
    originalPreferences = api.preferences;
  t.after(() => {
    api.me = originalMe;
    api.preferences = originalPreferences;
    disconnectPreferences();
  });
  let reads = 0;
  api.me = async () => {
    reads++;
    return {
      account_access: { id: "alice-id" },
      preferences: {},
      sync: { instance: "server", revision: reads },
    };
  };
  let finish;
  api.preferences = (values) =>
    new Promise((resolve) => {
      finish = () =>
        resolve({ values, sync: { instance: "server", revision: 2 } });
    });
  const snapshot = await loadInitialAccount();
  assert.equal(snapshot.sync.revision, 1);
  assert.equal(reads, 1);
  assert.equal(typeof finish, "function");
  finish();
});
