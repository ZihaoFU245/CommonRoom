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

test("slow upward scrolling releases bottom following inside the proximity threshold", () => {
  const viewport = {
    scrollHeight: 1600,
    scrollTop: 1000,
    clientHeight: 600,
    querySelector: () => null,
  };
  let following = true;
  for (let step = 0; step < 80; step++) {
    const previousTop = viewport.scrollTop;
    viewport.scrollTop -= 0.5;
    following = timelineAtBottom(viewport, following, previousTop);
    assert.equal(following, false);
    assert.equal(
      timelineAtBottom(viewport, following, viewport.scrollTop),
      false,
      "a duplicate scroll event cannot restore bottom following",
    );
  }
  const previousTop = viewport.scrollTop;
  viewport.scrollTop = 990;
  assert.equal(timelineAtBottom(viewport, following, previousTop), true);
});

test("a shrinking scroll extent retains following at the actual bottom", () => {
  const viewport = {
    scrollHeight: 1500,
    scrollTop: 900,
    clientHeight: 600,
    querySelector: () => null,
  };
  assert.equal(timelineAtBottom(viewport, true, 1000), true);
});
