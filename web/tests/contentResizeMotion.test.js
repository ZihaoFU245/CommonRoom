import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";

const registration = registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "preact/hooks")
      return { url: "test:resize-hooks", shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === "test:resize-hooks")
      return {
        format: "module",
        shortCircuit: true,
        source:
          "export const useContext = () => null; export const useRef = (value) => ({current:value}); export const useLayoutEffect = (callback) => globalThis.resizeEffect = callback;",
      };
    return next(url, context);
  },
});
const { useContentResizeMotion } =
  await import("../src/hooks/useContentResizeMotion.ts");
registration.deregister();

function fixture() {
  const originals = {
    window: globalThis.window,
    ResizeObserver: globalThis.ResizeObserver,
    requestAnimationFrame: globalThis.requestAnimationFrame,
    cancelAnimationFrame: globalThis.cancelAnimationFrame,
  };
  const listeners = new Map();
  const preference = {
    matches: false,
    addEventListener: (name, listener) => listeners.set(name, listener),
    removeEventListener: (name) => listeners.delete(name),
  };
  let notify,
    disconnected = false;
  globalThis.window = { matchMedia: () => preference };
  const frames = new Map();
  let nextFrame = 0;
  globalThis.requestAnimationFrame = (callback) => {
    frames.set(++nextFrame, callback);
    return nextFrame;
  };
  globalThis.cancelAnimationFrame = (id) => frames.delete(id);
  const viewportEvents = new Map();
  const viewport = {
    scrollHeight: 1000,
    clientHeight: 600,
    scrollTop: 0,
    getBoundingClientRect: () => ({ top: 0, bottom: 600 }),
    addEventListener: (event, callback) => viewportEvents.set(event, callback),
    removeEventListener: (event) => viewportEvents.delete(event),
  };
  globalThis.ResizeObserver = class {
    constructor(callback) {
      notify = callback;
    }
    observe() {}
    disconnect() {
      disconnected = true;
    }
  };
  const rect = { width: 300, height: 30, top: 100, bottom: 130 };
  let painted = 30,
    explicit = false;
  const animations = [];
  const hook = useContentResizeMotion();
  hook.content.current = {
    getBoundingClientRect: () => ({ ...rect }),
    querySelector: () => (explicit ? {} : null),
  };
  hook.panel.current = {
    dataset: {},
    getBoundingClientRect: () => ({ ...rect, height: painted }),
    closest: () => viewport,
    animate(frames, options) {
      const events = new Map();
      const animation = {
        frames,
        options,
        cancelled: false,
        cancel() {
          this.cancelled = true;
        },
        addEventListener: (name, callback) => events.set(name, callback),
        finish: () => events.get("finish")?.(),
      };
      animations.push(animation);
      return animation;
    },
  };
  const cleanup = globalThis.resizeEffect();
  return {
    animations,
    rect,
    preference,
    listeners,
    hook,
    viewport,
    viewportEvents,
    pendingFrames: () => frames.size,
    tick: () => {
      const current = [...frames.values()];
      frames.clear();
      for (const callback of current) callback();
    },
    resize(height) {
      rect.height = height;
      notify([
        {
          target: hook.content.current,
          borderBoxSize: [{ inlineSize: rect.width, blockSize: rect.height }],
        },
      ]);
    },
    paint(height) {
      painted = height;
    },
    explicit(value) {
      explicit = value;
    },
    close() {
      cleanup();
      assert.equal(disconnected, true);
      assert.equal(listeners.size, 0);
      Object.assign(globalThis, originals);
      delete globalThis.resizeEffect;
    },
  };
}

test("delayed content grows and shrinks at the shared timeline speed, without animating mount", () => {
  const f = fixture();
  try {
    f.resize(30);
    assert.equal(f.animations.length, 0);
    f.resize(210);
    const first = f.animations[0];
    assert.deepEqual(
      first.frames.map((frame) => frame.height),
      ["30px", "210px"],
    );
    assert.equal(first.options.duration, 180);
    assert.equal(first.options.easing, "ease-out");
    first.finish();
    assert.equal(first.cancelled, true);
    f.resize(60);
    assert.deepEqual(
      f.animations[1].frames.map((frame) => frame.height),
      ["210px", "60px"],
    );
  } finally {
    f.close();
  }
});

test("rapid loads retarget from the painted size and stale completion cannot cancel the new animation", () => {
  const f = fixture();
  try {
    f.resize(210);
    f.paint(90);
    f.resize(250);
    assert.equal(f.animations[0].cancelled, true);
    assert.equal(f.animations[1].frames[0].height, "90px");
    f.animations[0].finish();
    assert.equal(f.animations[1].cancelled, false);
    f.hook.stop();
    assert.equal(f.animations[1].cancelled, true);
  } finally {
    f.close();
  }
});

test("folding, width reflow, reduced motion and offscreen content do not replay height animations", () => {
  const f = fixture();
  try {
    f.explicit(true);
    f.resize(100);
    f.explicit(false);
    f.rect.width = 200;
    f.resize(120);
    f.preference.matches = true;
    f.resize(140);
    f.preference.matches = false;
    f.rect.top = 700;
    f.rect.bottom = 840;
    f.resize(160);
    assert.equal(f.animations.length, 0);
    f.rect.top = 100;
    f.resize(180);
    assert.equal(f.animations[0].frames[0].height, "160px");
    f.listeners.get("change")();
    assert.equal(f.animations[0].cancelled, true);
  } finally {
    f.close();
  }
});

test("bottom following takes ownership before the virtual list can anchor an intermediate height", () => {
  const f = fixture();
  try {
    // Natural content has already grown by 180px when ResizeObserver runs.
    f.viewport.scrollTop = 220;
    f.resize(210);
    assert.equal(f.viewport.scrollTop, 400);
    assert.equal(f.hook.panel.current.dataset.contentResizing, "");
    f.hook.stop();
    assert.equal(f.hook.panel.current.dataset.contentResizing, undefined);
  } finally {
    f.close();
  }
});

test("the final fold measurement cannot start a second resize animation", () => {
  const f = fixture();
  try {
    f.explicit(true);
    f.resize(300);
    f.explicit(false);
    f.resize(256);
    assert.equal(f.animations.length, 0);
    f.resize(280);
    assert.equal(f.animations[0].frames[0].height, "256px");
  } finally {
    f.close();
  }
});

test("manual scrolling releases bottom ownership without snapping to the end", () => {
  const f = fixture();
  try {
    f.viewport.scrollTop = 220;
    f.resize(210);
    assert.equal(f.pendingFrames(), 1);
    f.viewportEvents.get("wheel")();
    assert.equal(f.pendingFrames(), 0);
    f.viewport.scrollTop = 100;
    f.tick();
    assert.equal(f.viewport.scrollTop, 100);
    assert.equal(f.hook.panel.current.dataset.contentResizing, undefined);
  } finally {
    f.close();
  }
});

test("content still animates away from the bottom without a follow frame loop", () => {
  const f = fixture();
  try {
    f.resize(210);
    assert.equal(f.animations.length, 1);
    assert.equal(f.pendingFrames(), 0);
    assert.equal(f.viewport.scrollTop, 0);
    f.animations[0].finish();
    assert.equal(f.animations[0].cancelled, true);
  } finally {
    f.close();
  }
});
