import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";
let active;
const registration = registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "preact/hooks")
      return { url: "test:virtual-hooks", shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === "test:virtual-hooks")
      return {
        format: "module",
        shortCircuit: true,
        source:
          "export const useState = (...a) => globalThis.virtualTestHooks.state(...a); export const useRef = (...a) => globalThis.virtualTestHooks.ref(...a); export const useLayoutEffect = (...a) => globalThis.virtualTestHooks.effect(...a); export const useMemo = (...a) => globalThis.virtualTestHooks.memo(...a);",
      };
    return next(url, context);
  },
});
globalThis.virtualTestHooks = {
  memo(callback, deps) {
    const index = active.index++;
    const previous = active.slots[index];
    if (
      !previous ||
      deps === undefined ||
      deps.some((value, i) => !Object.is(value, previous.deps[i]))
    )
      active.slots[index] = { deps, value: callback() };
    return active.slots[index].value;
  },
  state(initial) {
    const index = active.index++;
    if (!(index in active.slots)) active.slots[index] = initial;
    const owner = active;
    return [
      owner.slots[index],
      (update) => {
        const value =
          typeof update === "function" ? update(owner.slots[index]) : update;
        if (!Object.is(value, owner.slots[index])) {
          owner.slots[index] = value;
          owner.dirty = true;
        }
      },
    ];
  },
  ref(initial) {
    const index = active.index++;
    return (active.slots[index] ??= { current: initial });
  },
  effect(callback, deps) {
    const index = active.index++;
    const previous = active.slots[index];
    if (
      !previous ||
      deps === undefined ||
      deps.some((value, i) => !Object.is(value, previous.deps[i]))
    ) {
      active.effects.push(() => {
        previous?.cleanup?.();
        active.slots[index] = { deps, cleanup: callback() };
      });
    }
  },
};
const { useVirtualTimeline } =
  await import("../src/hooks/useVirtualTimeline.ts");
registration.deregister();

function fixture(t) {
  const originals = {
    HTMLElement: globalThis.HTMLElement,
    ResizeObserver: globalThis.ResizeObserver,
    requestAnimationFrame: globalThis.requestAnimationFrame,
    cancelAnimationFrame: globalThis.cancelAnimationFrame,
  };
  const frames = new Map(),
    listeners = new Map(),
    observers = [],
    nodes = new Map();
  let frame = 0,
    rendered = [],
    reads = 0,
    renders = 0;
  class Row {
    constructor(key) {
      this.dataset = { virtualKey: key };
      this.height = 100;
    }
    getBoundingClientRect() {
      reads++;
      return { height: this.height };
    }
  }
  globalThis.HTMLElement = Row;
  globalThis.requestAnimationFrame = (callback) => {
    frames.set(++frame, callback);
    return frame;
  };
  globalThis.cancelAnimationFrame = (id) => frames.delete(id);
  globalThis.ResizeObserver = class {
    constructor(callback) {
      this.callback = callback;
      observers.push(this);
    }
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  let entries = Array.from({ length: 100 }, (_, i) => ({
    kind: "message",
    key: String(i + 1),
    message: { sequence: i + 1 },
  }));
  const viewport = {
    scrollTop: 0,
    clientHeight: 600,
    get scrollHeight() {
      return entries.reduce(
        (sum, e) => sum + (nodes.get(e.key)?.height ?? 100),
        0,
      );
    },
    querySelector: () => null,
    querySelectorAll: () => rendered,
    addEventListener: (event, cb) => listeners.set(event, cb),
    removeEventListener: (event) => listeners.delete(event),
  };
  const container = { current: viewport },
    handle = { current: null },
    bottom = { current: false };
  const harness = { slots: [], dirty: true };
  let result;
  function settle() {
    for (let i = 0; harness.dirty && i < 20; i++) {
      active = harness;
      harness.index = 0;
      harness.effects = [];
      harness.dirty = false;
      result = useVirtualTimeline(entries, container, handle, bottom);
      renders++;
      rendered = result.visible.map((e) => {
        if (!nodes.has(e.key)) nodes.set(e.key, new Row(e.key));
        return nodes.get(e.key);
      });
      // Browsers clamp scrollTop during DOM commit, before layout effects run.
      const committedHeight =
        result.top +
        result.bottom +
        rendered.reduce((sum, row) => sum + row.height, 0);
      viewport.scrollTop = Math.min(
        viewport.scrollTop,
        Math.max(0, committedHeight - viewport.clientHeight),
      );
      for (const effect of harness.effects) effect();
    }
    assert.equal(harness.dirty, false, "virtual timeline must settle");
  }
  function flush() {
    const work = [...frames.values()];
    frames.clear();
    for (const cb of work) cb();
    settle();
  }
  settle();
  flush();
  t.after(() => {
    for (const slot of harness.slots) slot?.cleanup?.();
    Object.assign(globalThis, originals);
  });
  return {
    viewport,
    handle,
    bottom,
    nodes,
    get result() {
      return result;
    },
    get renders() {
      return renders;
    },
    get reads() {
      return reads;
    },
    scroll(top) {
      viewport.scrollTop = top;
      listeners.get("scroll")();
      flush();
    },
    prepend() {
      entries = [
        { kind: "message", key: "older", message: { sequence: 0 } },
        ...entries,
      ];
      harness.dirty = true;
      settle();
    },
    replace(start, end) {
      entries = entries.filter(
        (entry) =>
          entry.message.sequence >= start && entry.message.sequence <= end,
      );
      harness.dirty = true;
      settle();
    },
    resize(key, height) {
      const target = nodes.get(key);
      target.height = height;
      observers[1].callback([
        { target, borderBoxSize: [{ inlineSize: 320, blockSize: height }] },
      ]);
      settle();
    },
    resizeViewport(height) {
      viewport.clientHeight = height;
      observers[0].callback();
      flush();
    },
  };
}

test("pixel scrolling avoids renders until the mounted window changes", (t) => {
  const f = fixture(t);
  const renders = f.renders,
    reads = f.reads;
  for (let top = 1; top < 100; top++) f.scroll(top);
  assert.equal(f.renders, renders);
  assert.equal(f.reads, reads);
  f.scroll(100);
  assert.ok(f.renders > renders);
  assert.equal(f.result.end, 16);
  f.scroll(150);
  f.prepend();
  assert.equal(
    f.viewport.scrollTop,
    250,
    "paging anchors the latest real scroll position",
  );
  assert.deepEqual(f.handle.current.position(), { sequence: 2, offset: -50 });
});

test("background history replacement preserves its anchor before DOM scroll clamping", (t) => {
  const f = fixture(t);
  f.scroll(4917);
  assert.deepEqual(f.handle.current.position(), { sequence: 50, offset: -17 });
  f.replace(40, 60);
  assert.deepEqual(f.handle.current.position(), { sequence: 50, offset: -17 });
  assert.equal(f.viewport.scrollTop, 1017);
});

test("shrinking a history window cannot jump backward to an older retained message", (t) => {
  const f = fixture(t);
  f.scroll(4917);
  f.replace(10, 60);
  assert.deepEqual(f.handle.current.position(), { sequence: 50, offset: -17 });
});

test("observer dimensions anchor resized rows without extra rect reads and retain bottom following", (t) => {
  const f = fixture(t);
  f.scroll(150);
  const reads = f.reads;
  f.resize("1", 150);
  assert.equal(f.viewport.scrollTop, 200);
  assert.equal(f.reads, reads);
  f.resizeViewport(500);
  assert.equal(f.viewport.scrollTop, 200);
  f.bottom.current = true;
  f.scroll(f.viewport.scrollHeight - f.viewport.clientHeight);
  f.resize("100", 180);
  assert.equal(
    f.viewport.scrollTop,
    f.viewport.scrollHeight - f.viewport.clientHeight,
  );
});

test("anchoring refreshes a temporarily changed window even when it returns to previous boundaries", (t) => {
  const f = fixture(t);
  f.scroll(150);
  const end = f.result.end;
  f.resize("1", 200);
  assert.equal(f.viewport.scrollTop, 250);
  assert.equal(
    f.result.end,
    end,
    "mount the anchored window, not the pre-anchor window",
  );
});
