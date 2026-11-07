import assert from "node:assert/strict";
import test from "node:test";
import { createHookHarness, mockHooks, useHarness } from "./helpers/hooks.js";

const registration = mockHooks();
const { useMessageExpansionMotion } =
  await import("../src/hooks/useMessageExpansionMotion.ts");
registration.deregister();

test("preview height changes remeasure mounted content and maximum disables folding", (t) => {
  const previous = globalThis.ResizeObserver;
  let notify;
  let disconnects = 0;
  globalThis.ResizeObserver = class {
    constructor(callback) {
      notify = callback;
    }
    observe() {}
    disconnect() {
      disconnects++;
    }
  };
  t.after(() => {
    globalThis.ResizeObserver = previous;
  });
  const harness = createHookHarness();
  let height = 500;
  const element = { getBoundingClientRect: () => ({ height }) };
  function render(limit) {
    useHarness(harness);
    const hook = useMessageExpansionMotion(limit);
    hook.content.current = element;
    hook.panel.current = {};
    for (const effect of harness.effects.splice(0)) effect();
    useHarness(harness);
    return useMessageExpansionMotion(limit);
  }
  assert.equal(render(256).overflows, true);
  assert.equal(render(640).overflows, false);
  height = 900;
  notify([{ borderBoxSize: [{ inlineSize: 300, blockSize: height }] }]);
  assert.equal(render(640).overflows, true);
  assert.equal(render(null).overflows, false);
  height = 5000;
  notify([{ borderBoxSize: [{ inlineSize: 300, blockSize: height }] }]);
  assert.equal(render(null).overflows, false);
  assert.equal(render(256).overflows, true);
  assert.equal(disconnects, 3);
});
