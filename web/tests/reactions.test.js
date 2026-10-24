import assert from "node:assert/strict";
import test from "node:test";
import {
  orderReactions,
  reactionChoices,
  rememberReaction,
} from "../src/features/preferences/reactions.ts";

test("reaction choices keep the default order without recent use", () => {
  assert.deepEqual(orderReactions([]), reactionChoices);
  assert.equal(new Set(reactionChoices).size, reactionChoices.length);
});

test("recent reactions move to the front without duplicates", () => {
  const recent = rememberReaction(rememberReaction([], "🔥"), "👍");
  assert.deepEqual(recent, ["👍", "🔥"]);
  const ordered = orderReactions(recent);
  assert.deepEqual(ordered.slice(0, 3), ["👍", "🔥", "🍼"]);
  assert.equal(ordered.length, reactionChoices.length);
});

test("reusing a reaction moves it to the front and custom text is kept", () => {
  assert.deepEqual(rememberReaction(["👍", "🔥", "ok"], "🔥"), [
    "🔥",
    "👍",
    "ok",
  ]);
  assert.deepEqual(rememberReaction(["👍"], " lol "), ["lol", "👍"]);
  assert.deepEqual(rememberReaction(["👍"], "  "), ["👍"]);
  assert.deepEqual(orderReactions(["lol"])[0], "lol");
});

test("recent reactions are capped", () => {
  let recent = [];
  for (let index = 0; index < 40; index += 1)
    recent = rememberReaction(recent, `r${index}`);
  assert.equal(recent.length, 24);
  assert.equal(recent[0], "r39");
});
