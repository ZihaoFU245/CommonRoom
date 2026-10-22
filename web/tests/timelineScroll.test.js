import assert from "node:assert/strict";
import test from "node:test";
import { timelineAtBottom } from "../src/features/conversation/timelineScroll.ts";

test("animation overflow cannot release bottom-following or repin an older reader", () => {
  let animating = true;
  const viewport = {
    scrollHeight: 1259,
    scrollTop: 400,
    clientHeight: 600,
    querySelector: () => (animating ? {} : null),
  };
  assert.equal(timelineAtBottom(viewport, true), true);
  assert.equal(timelineAtBottom(viewport, false), false);
  // Manual input cancels timeline/resize ownership before its scroll event.
  animating = false;
  assert.equal(timelineAtBottom(viewport, true), false);
  viewport.scrollTop = 659;
  assert.equal(timelineAtBottom(viewport, false), true);
});
