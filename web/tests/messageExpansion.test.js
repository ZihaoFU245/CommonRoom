import test from "node:test";
import assert from "node:assert/strict";
import { messageExpansionMotion } from "../src/features/conversation/messageExpansion.ts";

test("long expansions spend most of the duration inside the visible viewport", () => {
  for (const fullHeight of [3338, 5000, 12000]) {
    const motion = messageExpansionMotion(256, fullHeight, 480);
    assert.equal(motion.frames[0].height, "256px");
    assert.equal(motion.frames[1].height, "480px");
    assert.equal(motion.frames[1].offset, 0.8);
    assert.equal(motion.frames.at(-1).height, `${fullHeight}px`);
    assert.ok(motion.duration * motion.frames[1].offset >= 300);
  }
});

test("short expansions and footer-pinned collapses retain a continuous transition", () => {
  for (const [from, to] of [
    [256, 400],
    [3338, 256],
    [5000, 256],
  ]) {
    const motion = messageExpansionMotion(from, to, 480);
    assert.equal(motion.frames.length, 2);
    assert.equal(motion.frames[0].height, `${from}px`);
    assert.equal(motion.frames[1].height, `${to}px`);
    assert.equal(motion.duration, 300);
  }
});

test("an interrupted expansion starts at its current height without reversing", () => {
  const motion = messageExpansionMotion(600, 3338, 400);
  assert.equal(motion.frames[0].height, "600px");
  assert.ok(parseFloat(motion.frames[1].height) > 600);
  assert.equal(motion.frames.at(-1).height, "3338px");
});
