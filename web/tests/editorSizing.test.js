import assert from "node:assert/strict";
import test from "node:test";
import { editorScrollTop } from "../src/features/conversation/editorSizing.ts";

test("growth and shrinkage preserve the footer's screen position", () => {
  for (const [top, oldHeight, height] of [
    [600, 100, 124],
    [624, 124, 100],
    [600, 100, 500],
    [1000, 500, 100],
  ]) {
    const next = editorScrollTop(top, oldHeight, height, 2000);
    const footerOffset = 900;
    assert.equal(footerOffset + height - oldHeight - next, footerOffset - top);
  }
});

test("anchoring restores the old scroll offset after temporary measurement collapse", () => {
  // Setting textarea height to zero can clamp the browser's scrollTop before
  // measurement; the calculation must use the offset captured beforehand.
  assert.equal(editorScrollTop(900, 1100, 1150, 950), 950);
});

test("scroll anchoring respects the available scroll range", () => {
  assert.equal(editorScrollTop(10, 200, 20, 100), 0);
  assert.equal(editorScrollTop(90, 20, 200, 100), 100);
  assert.equal(editorScrollTop(0, 20, 200, -100), 0);
  assert.equal(editorScrollTop(50, 100, 100, 200), 50);
});
