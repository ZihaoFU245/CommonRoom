import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, mockHooks, useHarness } from "./helpers/hooks.js";
const registration = mockHooks();
const { useAppViewport } = await import("../src/hooks/useAppViewport.ts");
registration.deregister();

test("keyboard viewport resize and pan are batched and leave pinch zoom alone", (t) => {
  const visual = Object.assign(new EventTarget(), {
    height: 800,
    offsetTop: 0,
    scale: 1,
  });
  const window = Object.assign(new EventTarget(), {
    visualViewport: visual,
    innerHeight: 900,
  });
  const properties = new Map();
  const frames = new Map();
  let serial = 0;
  const values = {
    window,
    document: {
      documentElement: {
        style: {
          setProperty: (key, value) => properties.set(key, value),
          removeProperty: (key) => properties.delete(key),
        },
      },
    },
    requestAnimationFrame: (callback) => {
      frames.set(++serial, callback);
      return serial;
    },
    cancelAnimationFrame: (id) => frames.delete(id),
  };
  for (const [key, value] of Object.entries(values)) {
    const original = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, { configurable: true, value });
    t.after(() => {
      if (original) Object.defineProperty(globalThis, key, original);
      else delete globalThis[key];
    });
  }
  const harness = createHookHarness();
  useHarness(harness);
  useAppViewport();
  harness.effects.shift()();
  const flush = () => {
    const callbacks = [...frames.values()];
    frames.clear();
    for (const callback of callbacks) callback();
  };
  assert.equal(properties.get("--app-height"), "800px");
  assert.equal(properties.get("--app-top"), "0px");
  visual.height = 450;
  for (let i = 0; i < 10; i++) visual.dispatchEvent(new Event("resize"));
  visual.offsetTop = 350;
  visual.dispatchEvent(new Event("scroll"));
  assert.equal(frames.size, 1);
  flush();
  assert.equal(properties.get("--app-height"), "450px");
  assert.equal(properties.get("--app-top"), "350px");
  visual.offsetTop = 120;
  visual.dispatchEvent(new Event("scroll"));
  flush();
  assert.equal(properties.get("--app-top"), "120px");
  visual.scale = 2;
  visual.height = 200;
  visual.offsetTop = 200;
  visual.dispatchEvent(new Event("resize"));
  flush();
  assert.equal(properties.get("--app-height"), "450px");
  assert.equal(properties.get("--app-top"), "120px");
  Object.assign(visual, { height: 800, offsetTop: 0, scale: 1 });
  visual.dispatchEvent(new Event("resize"));
  flush();
  assert.equal(properties.get("--app-height"), "800px");
  assert.equal(properties.get("--app-top"), "0px");
  harness.slots[0].cleanup();
  visual.dispatchEvent(new Event("resize"));
  visual.dispatchEvent(new Event("scroll"));
  window.dispatchEvent(new Event("resize"));
  assert.equal(frames.size, 0);
  assert.equal(properties.has("--app-height"), false);
  assert.equal(properties.has("--app-top"), false);
});
