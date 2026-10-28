import assert from "node:assert/strict";
import test from "node:test";
import {
  bottomShortcut,
  UnreadJumpTargets,
  retainedJumpSequence,
} from "../src/features/conversation/historyNavigation.ts";

test("bottom shortcut uses three-screen distance without a speed trigger", () => {
  assert.equal(bottomShortcut(601, 600, false, false), false);
  assert.equal(bottomShortcut(1799, 600, false, false), false);
  assert.equal(bottomShortcut(1800, 600, false, false), true);
  assert.equal(bottomShortcut(601, 600, true, false), false);
  assert.equal(bottomShortcut(300, 600, false, true), true);
  assert.equal(bottomShortcut(20, 600, false, true), false);
  assert.equal(bottomShortcut(0, 600, true, false), false);
});

test("deleted and expired jump targets resolve to retained messages", () => {
  assert.equal(retainedJumpSequence([88, 89, 91, 92], 90), 91);
  assert.equal(retainedJumpSequence([88, 89, 91, 92], 89), 89);
  assert.equal(retainedJumpSequence([100, 101], 50), 100);
  assert.equal(retainedJumpSequence([88, 89], 100), 89);
  assert.equal(retainedJumpSequence([], 100), undefined);
});

test("unread jump is stable and usable once until the unread batch clears", () => {
  const jumps = new UnreadJumpTargets();
  assert.equal(jumps.target("a", 50, 100), 50);
  assert.equal(
    jumps.target("a", 60, 90),
    50,
    "read acknowledgements cannot move the target",
  );
  assert.equal(jumps.target("b", 10, 20), 10);
  assert.equal(jumps.consume("a"), 50);
  assert.equal(
    jumps.consume("a"),
    undefined,
    "duplicate clicks cannot skip ahead",
  );
  assert.equal(jumps.target("a", 70, 80), undefined);
  assert.equal(
    jumps.target("b", 11, 19),
    10,
    "rooms keep independent destinations",
  );
  assert.equal(jumps.target("a", null, 0), undefined);
  assert.equal(
    jumps.target("a", 200, 1),
    200,
    "a later unread batch can offer a new jump",
  );
});
