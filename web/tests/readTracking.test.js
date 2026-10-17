import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";
import { api } from "../src/api/client.ts";

let index = 0;
const slots = [];
const effects = [];
globalThis.readTestHooks = {
  ref(value) {
    return (slots[index++] ??= { current: value });
  },
  state(value) {
    const i = index++;
    if (!(i in slots)) slots[i] = value;
    return [
      slots[i],
      (v) => {
        slots[i] = v;
      },
    ];
  },
  callback(fn) {
    return (slots[index++] ??= fn);
  },
  effect(fn, deps) {
    const i = index++;
    const previous = slots[i];
    if (!previous || deps.some((v, j) => !Object.is(v, previous.deps[j]))) {
      effects.push(() => {
        previous?.cleanup?.();
        slots[i] = { deps, cleanup: fn() };
      });
    }
  },
};
const registration = registerHooks({
  resolve(specifier, context, next) {
    return specifier === "preact/hooks"
      ? { url: "test:read-hooks", shortCircuit: true }
      : next(specifier, context);
  },
  load(url, context, next) {
    return url === "test:read-hooks"
      ? {
          format: "module",
          shortCircuit: true,
          source:
            "export const useRef=(...a)=>globalThis.readTestHooks.ref(...a); export const useState=(...a)=>globalThis.readTestHooks.state(...a); export const useCallback=(...a)=>globalThis.readTestHooks.callback(...a); export const useEffect=(...a)=>globalThis.readTestHooks.effect(...a); export const useLayoutEffect=useEffect;",
        }
      : next(url, context);
  },
});
const { useReadTracking } = await import("../src/hooks/useReadTracking.ts");
registration.deregister();

test("continuous snapshots retain one read timer and acknowledge the latest visible sequence", async (t) => {
  const originals = {
    document: globalThis.document,
    window: globalThis.window,
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
    read: api.read,
  };
  t.after(() => {
    for (const slot of slots) slot?.cleanup?.();
    globalThis.document = originals.document;
    globalThis.window = originals.window;
    globalThis.setTimeout = originals.setTimeout;
    globalThis.clearTimeout = originals.clearTimeout;
    api.read = originals.read;
    delete globalThis.readTestHooks;
  });
  const timers = new Map();
  let nextTimer = 0;
  let sequence = 1;
  globalThis.setTimeout = (callback) => {
    timers.set(++nextTimer, callback);
    return nextTimer;
  };
  globalThis.clearTimeout = (id) => timers.delete(id);
  globalThis.document = {
    hidden: false,
    hasFocus: () => true,
    addEventListener() {},
    removeEventListener() {},
  };
  globalThis.window = { addEventListener() {}, removeEventListener() {} };
  const reads = [];
  api.read = async (view, through) => {
    reads.push([view, through]);
  };
  const viewport = {
    querySelectorAll: () => [
      {
        dataset: { sequence: String(sequence) },
        getBoundingClientRect: () => ({ top: 10, bottom: 20 }),
      },
    ],
    getBoundingClientRect: () => ({ top: 0, bottom: 100 }),
    scrollTop: 0,
    scrollHeight: 100,
    clientHeight: 100,
  };
  const context = {
    selected: "team",
    unread: { first: null },
    messages: [],
    historyReady: true,
    output: [],
    cleared: {},
    resetCleared() {},
    settingsOpen: false,
    menu: false,
    status: "online",
    loadSequence() {},
    virtualTimeline: { current: null },
    atBottom: { current: true },
    historyElement: { current: viewport },
  };
  for (sequence = 1; sequence <= 20; sequence++) {
    index = 0;
    effects.length = 0;
    useReadTracking({ ...context, messages: [{ sequence }] });
    for (const effect of effects) effect();
    assert.equal(timers.size, 1);
  }
  const callback = [...timers.values()][0];
  timers.clear();
  callback();
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(reads, [["team", 21]]);
});
