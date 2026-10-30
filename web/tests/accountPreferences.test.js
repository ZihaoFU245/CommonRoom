import assert from "node:assert/strict";
import test from "node:test";
import {
  connectPreferences,
  disconnectPreferences,
  changePreference,
  flushPreferences,
  preferenceValue,
  preferenceError,
  receivePreferences,
} from "../src/features/preferences/account.ts";

function fixture(t) {
  const saved = new Map();
  const descriptor = Object.getOwnPropertyDescriptor(
    globalThis,
    "localStorage",
  );
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key) => saved.get(key) ?? null,
      setItem: (key, value) => saved.set(key, value),
    },
  });
  t.after(() => {
    disconnectPreferences();
    if (descriptor)
      Object.defineProperty(globalThis, "localStorage", descriptor);
    else delete globalThis.localStorage;
  });
  return saved;
}
function cloud() {
  let values = {};
  let revision = 0;
  return async (patch, initialize) => {
    for (const [key, value] of Object.entries(patch)) {
      if (!initialize || values[key] === undefined) {
        if (value === null) delete values[key];
        else values[key] = value;
      }
    }
    revision++;
    return { values: { ...values }, sync: { instance: "server", revision } };
  };
}

test("first device seeds local settings; subsequent login pulls cloud values and fills only unset fields", async (t) => {
  const local = fixture(t);
  const transport = cloud();
  local.set("commonroom-theme", "dark");
  local.set("commonroom-name-colors", "on");
  local.set("commonroom-fonts", '{"chat":22,"ui":17}');
  local.set("commonroom-notifications", "on");
  await connectPreferences("alice", transport);
  assert.equal(preferenceValue("theme", ""), "dark");
  assert.equal(preferenceValue("name_colors", ""), "true");
  assert.equal(preferenceValue("font.chat", ""), "22");
  // Notification preference imports even on a device with no Notification API.
  assert.equal(preferenceValue("notifications", ""), "true");
  disconnectPreferences();
  local.clear();
  local.set("commonroom-theme", "light");
  await connectPreferences("alice", transport);
  assert.equal(preferenceValue("theme", ""), "dark");
  assert.equal(preferenceValue("font.chat", ""), "22");
  disconnectPreferences();
  await transport({ theme: null }, false);
  local.clear();
  local.set("commonroom-theme", "custom");
  await connectPreferences("alice", transport);
  assert.equal(preferenceValue("theme", ""), "custom");
  assert.equal(preferenceValue("font.chat", ""), "22");
});

test("patches retain concurrent device updates, pending edits and reject stale responses", async (t) => {
  fixture(t);
  const transport = cloud();
  let finish;
  await connectPreferences("alice", async (patch, initialize) => {
    if (initialize) return transport(patch, true);
    return new Promise((resolve) => {
      finish = resolve;
    });
  });
  changePreference("theme", "dark");
  const flushing = flushPreferences();
  const newer = await transport({ theme: "light", latex: "true" }, false);
  receivePreferences(newer);
  assert.equal(
    preferenceValue("theme", ""),
    "dark",
    "keep optimistic edit until acknowledged",
  );
  assert.equal(preferenceValue("latex", ""), "true");
  finish({
    values: { theme: "dark" },
    sync: { instance: "server", revision: 1 },
  });
  await flushing;
  assert.equal(
    preferenceValue("theme", ""),
    "light",
    "newer authoritative change survives older write response",
  );
  assert.equal(preferenceValue("latex", ""), "true");
});

test("failed writes remain pending for retry; logout isolates in-flight completion and cached accounts", async (t) => {
  const local = fixture(t);
  const transport = cloud();
  let fail = true;
  await connectPreferences("alice", async (patch, initialize) => {
    if (!initialize && fail) throw Error("offline");
    return transport(patch, initialize);
  });
  changePreference("theme", "dark");
  await flushPreferences();
  assert.match(preferenceError(), /could not sync/u);
  assert.equal(preferenceValue("theme", ""), "dark");
  fail = false;
  await flushPreferences();
  assert.equal(preferenceError(), "");
  assert.equal(
    JSON.parse(local.get("commonroom-preferences:alice")).theme,
    "dark",
  );
  disconnectPreferences();
  await connectPreferences("bob", cloud());
  assert.equal(preferenceValue("theme", ""), "system");
});

test("blocked local storage still seeds defaults and receives server updates", async (t) => {
  fixture(t);
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get() {
      throw Error("blocked");
    },
  });
  await connectPreferences("alice", cloud());
  assert.equal(preferenceValue("theme", ""), "system");
  changePreference("name_colors", "true");
  await flushPreferences();
  assert.equal(preferenceValue("name_colors", ""), "true");
});
