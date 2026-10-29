import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, mockHooks, useHarness } from "./helpers/hooks.js";
import { api } from "../src/api/client.ts";
const registration = mockHooks();
const { useBotCommands } = await import("../src/hooks/useBotCommands.ts");
registration.deregister();

test("conversation frame churn preserves discovery timers and reads the latest directory", async (t) => {
  const originalWindow = globalThis.window;
  const originalDocument = globalThis.document;
  const originalFetch = api.botCommands;
  const timeouts = new Map(),
    intervals = new Map();
  let serial = 0;
  const calls = [];
  globalThis.document = { hidden: false };
  globalThis.window = {
    setTimeout(callback) {
      timeouts.set(++serial, callback);
      return serial;
    },
    clearTimeout(id) {
      timeouts.delete(id);
    },
    setInterval(callback) {
      intervals.set(++serial, callback);
      return serial;
    },
    clearInterval(id) {
      intervals.delete(id);
    },
    addEventListener() {},
    removeEventListener() {},
  };
  api.botCommands = async () => {
    calls.push(true);
    return { conversations: [] };
  };
  t.after(() => {
    globalThis.window = originalWindow;
    globalThis.document = originalDocument;
    api.botCommands = originalFetch;
  });
  const harness = createHookHarness();
  const render = (version) => {
    useHarness(harness);
    useBotCommands(
      "a",
      new Map([["a", version]]),
      "authority",
      new Map([["a", "members"]]),
      true,
    );
    for (const effect of harness.effects.splice(0)) effect();
  };
  render(0);
  const initialTimer = [...timeouts.keys()][0];
  const periodicTimer = [...intervals.keys()][0];
  for (let version = 1; version <= 20; version++) render(version);
  assert.ok(
    timeouts.has(initialTimer),
    "initial timer must not be postponed by frames",
  );
  assert.ok(
    intervals.has(periodicTimer),
    "periodic timer must retain its deadline",
  );
  timeouts.get(initialTimer)();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 1);
  render(21);
  // The stable timer must capture version 21 rather than the original version 0.
  intervals.get(periodicTimer)();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls.length, 2);
  for (const slot of harness.slots) slot?.cleanup?.();
});
