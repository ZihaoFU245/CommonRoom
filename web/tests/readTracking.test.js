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

test("conversation positions restore sequence and pixel offset by stable identity", (t) => {
  index = 0;
  slots.length = 0;
  effects.length = 0;
  const originals = {
    window: globalThis.window,
    document: globalThis.document,
  };
  globalThis.window = { addEventListener() {}, removeEventListener() {} };
  globalThis.document = {
    hidden: false,
    hasFocus: () => false,
    addEventListener() {},
    removeEventListener() {},
  };
  t.after(() => {
    for (const slot of slots) slot?.cleanup?.();
    Object.assign(globalThis, originals);
  });
  const calls = [];
  let sequence = 20,
    offset = -13;
  const bottom = { current: true };
  const context = {
    selected: "a",
    conversationId: "a-id",
    unread: { first: null, oldest: 1, through: 100 },
    messages: [{ sequence: 20 }],
    historyReady: true,
    output: [],
    cleared: {},
    resetCleared() {},
    settingsOpen: false,
    menu: false,
    status: "online",
    loadSequence() {
      assert.fail("cached position should not need HTTP");
    },
    virtualTimeline: {
      current: {
        position: () => ({ sequence, offset }),
        anchor: () => sequence,
        scrollToSequence: (...args) => calls.push(args),
      },
    },
    atBottom: bottom,
    historyElement: {
      current: { scrollTop: 0, scrollHeight: 1000, clientHeight: 200 },
    },
  };
  function render() {
    index = 0;
    effects.length = 0;
    const result = useReadTracking(context);
    for (const effect of effects) effect();
    return result;
  }
  let result = render();
  bottom.current = false;
  result.rememberPosition();
  context.selected = "b";
  context.conversationId = "b-id";
  context.messages = [{ sequence: 30 }];
  sequence = 30;
  offset = 0;
  render();
  context.selected = "a";
  context.conversationId = "a-id";
  context.messages = [{ sequence: 20 }];
  render();
  assert.deepEqual(calls, [[20, -13]]);
  assert.equal(bottom.current, false);
  calls.length = 0;
  context.selected = "b";
  context.conversationId = "b-id";
  context.messages = [{ sequence: 30 }];
  render();
  context.selected = "a";
  context.conversationId = "a-id";
  context.messages = [{ sequence: 19 }, { sequence: 21 }];
  render();
  assert.deepEqual(
    calls,
    [[21, -13]],
    "a retracted anchor uses its retained neighbor without repeated paging",
  );
  calls.length = 0;
  context.conversationId = "replacement-id";
  render();
  assert.deepEqual(
    calls,
    [],
    "a recreated conversation must not inherit the old position",
  );
});
