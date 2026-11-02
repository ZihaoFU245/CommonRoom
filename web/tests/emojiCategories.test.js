import test from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_EMOJI_CATEGORY,
  EMOJI_CATEGORIES,
} from "../src/features/conversation/emojiCategories.ts";
import { EMOJIS } from "../src/features/conversation/emojis.ts";

test("emoji categories cover the curated list without duplicates", () => {
  assert.equal(DEFAULT_EMOJI_CATEGORY, "faces");
  assert.ok(EMOJI_CATEGORIES.length >= 8);
  const seen = new Set();
  let total = 0;
  for (const category of EMOJI_CATEGORIES) {
    assert.ok(category.emojis.length > 0, `${category.id} is empty`);
    assert.ok(category.icon, `${category.id} needs an icon`);
    assert.ok(category.label, `${category.id} needs a label`);
    for (const emoji of category.emojis) {
      assert.equal(seen.has(emoji), false, `duplicate ${emoji}`);
      seen.add(emoji);
      total += 1;
    }
  }
  assert.equal(total, EMOJIS.length);
  assert.equal(seen.size, EMOJIS.length);
  for (const emoji of EMOJIS) assert.ok(seen.has(emoji));
});

test("emoji categories keep faces, gestures, and people separate", () => {
  const byId = Object.fromEntries(
    EMOJI_CATEGORIES.map((category) => [category.id, category]),
  );
  assert.ok(byId.faces.emojis.includes("😀"));
  assert.ok(byId.gestures.emojis.includes("👍"));
  assert.ok(byId.people.emojis.includes("🧑"));
  assert.ok(
    byId.nature.emojis.includes("🐶") || byId.nature.emojis.includes("🌿"),
  );
  assert.ok(byId.food.emojis.includes("🍔") || byId.food.emojis.includes("🍎"));
  assert.ok(
    byId.places.emojis.includes("🏠") || byId.places.emojis.includes("✈️"),
  );
});
