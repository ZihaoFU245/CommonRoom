import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";

const registration = registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "preact/hooks")
      return { url: "test:timeline-scroll-hooks", shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === "test:timeline-scroll-hooks")
      return {
        format: "module",
        shortCircuit: true,
        source: `
          export const useRef = (value) => ({ current: value });
          export const useCallback = callback => callback;
          export const useEffect = (callback) => globalThis.scrollEffect = callback;
          export const useLayoutEffect = (callback) => globalThis.scrollLayout = callback;
        `,
      };
    return next(url, context);
  },
});
const { useTimelineMotion } = await import("../src/hooks/useTimelineMotion.ts");
registration.deregister();

test("idle wheel/touch events do not measure rows and scrolling keeps the cached baseline", async (t) => {
  const events = new Map();
  let measurements = 0;
  let resized;
  const viewport = {
    scrollTop: 0,
    getBoundingClientRect: () => {
      measurements++;
      return { top: 0, left: 0 };
    },
    querySelectorAll: () => [],
    querySelector: () => null,
    addEventListener: (name, callback) => events.set(name, callback),
    removeEventListener: (name) => events.delete(name),
  };
  const originals = {
    window: globalThis.window,
    ResizeObserver: globalThis.ResizeObserver,
  };
  globalThis.window = {
    matchMedia: () => ({ addEventListener() {}, removeEventListener() {} }),
  };
  globalThis.ResizeObserver = class {
    constructor(callback) {
      resized = callback;
    }
    observe() {}
    disconnect() {}
  };
  t.after(() => {
    Object.assign(globalThis, originals);
    delete globalThis.scrollEffect;
    delete globalThis.scrollLayout;
  });
  useTimelineMotion(
    [],
    "room",
    1,
    true,
    { current: viewport },
    { current: false },
    {
      start: 0,
      end: 0,
      layoutVersion: 0,
    },
  );
  const cleanup = globalThis.scrollEffect();
  t.after(cleanup);
  globalThis.scrollLayout();
  await Promise.resolve();
  assert.equal(measurements, 1);
  for (let index = 0; index < 120; index++) {
    events.get("wheel")();
    viewport.scrollTop++;
    events.get("scroll")();
  }
  events.get("touchstart")();
  events.get("pointerdown")();
  events.get("keydown")();
  assert.equal(measurements, 1);
  resized();
  assert.equal(measurements, 2);
});

test("motion reuses row rectangles from this commit and remeasures after scrolling", async (t) => {
  let reads = 0;
  const rowRect = { top: 0, left: 0, width: 100, height: 50 };
  const row = {
    dataset: { virtualKey: "m" },
    getBoundingClientRect() {
      reads++;
      return rowRect;
    },
    querySelector: () => null,
    getAnimations: () => [],
  };
  const viewport = {
    scrollTop: 0,
    getBoundingClientRect: () => ({ top: 0, left: 0 }),
    querySelectorAll: () => [row],
    querySelector: () => null,
  };
  t.after(() => {
    delete globalThis.scrollLayout;
  });
  useTimelineMotion(
    [],
    "room",
    1,
    true,
    { current: viewport },
    { current: false },
    {
      start: 0,
      end: 1,
      layoutVersion: 0,
      rowMeasurements: { rects: new Map([[row, rowRect]]), scrollTop: 0 },
    },
  );
  globalThis.scrollLayout();
  await Promise.resolve();
  assert.equal(reads, 0, "no duplicate per-row layout read");
  viewport.scrollTop = 10;
  globalThis.scrollLayout();
  await Promise.resolve();
  assert.equal(
    reads,
    1,
    "a changed scroll position invalidates shared coordinates",
  );
});
