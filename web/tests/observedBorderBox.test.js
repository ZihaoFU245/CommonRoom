import assert from "node:assert/strict";
import test from "node:test";
import { observedBorderBox } from "../src/features/conversation/observedBorderBox.ts";

test("observer border dimensions avoid geometry reads and include padding/borders", () => {
  assert.deepEqual(
    observedBorderBox({
      borderBoxSize: [{ inlineSize: 320, blockSize: 120 }],
      contentRect: { width: 300, height: 100 },
      target: {
        getBoundingClientRect: () => assert.fail("unnecessary layout read"),
      },
    }),
    { width: 320, height: 120 },
  );
});

test("missing border sizes use border-box geometry instead of contentRect", () => {
  for (const borderBoxSize of [
    undefined,
    [],
    [{ inlineSize: NaN, blockSize: 100 }],
  ]) {
    const rect = { width: 320, height: 120 };
    assert.equal(
      observedBorderBox({
        borderBoxSize,
        contentRect: { width: 300, height: 100 },
        target: { getBoundingClientRect: () => rect },
      }),
      rect,
    );
  }
});
