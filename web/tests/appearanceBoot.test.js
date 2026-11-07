import assert from "node:assert/strict";
import test from "node:test";
import { bootAppearance } from "../src/features/preferences/appearance.ts";
import { derivePalette } from "../src/features/preferences/palette.ts";

test("appearance bootstrap applies saved fonts and custom palette before render", (t) => {
  const properties = new Map();
  const root = {
    dataset: {},
    style: {
      setProperty: (key, value) => properties.set(key, value),
      removeProperty: (key) => properties.delete(key),
    },
  };
  const entries = new Map([
    ["commonroom-fonts", '{"chat":22,"ui":18}'],
    ["commonroom-theme", "custom"],
    ["commonroom-palette", '{"base":"#123456","accent":"#abcdef"}'],
  ]);
  for (const [key, value] of Object.entries({
    document: { documentElement: root, querySelector: () => null },
    window: { matchMedia: () => ({ matches: true }) },
    localStorage: { getItem: (key) => entries.get(key) ?? null },
  })) {
    const original = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, { configurable: true, value });
    t.after(() => {
      if (original) Object.defineProperty(globalThis, key, original);
      else delete globalThis[key];
    });
  }
  bootAppearance();
  assert.equal(properties.get("--chat-font-size"), "22px");
  assert.equal(properties.get("--ui-font-size"), "18px");
  const palette = derivePalette("#123456", "#abcdef");
  assert.equal(root.dataset.theme, palette.polarity);
  for (const [key, value] of Object.entries(palette.tokens))
    assert.equal(properties.get(`--${key}`), value);
  globalThis.localStorage.getItem = () => {
    throw new Error("Unexpected storage failure");
  };
  assert.doesNotThrow(bootAppearance);
  assert.equal(root.dataset.theme, "dark");
  assert.equal(properties.get("--chat-font-size"), "16px");
  assert.equal(properties.get("--ui-font-size"), "14px");
  assert.equal(properties.has("--paper"), false);
});
