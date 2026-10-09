import assert from "node:assert/strict";
import test from "node:test";
import { defaultSeeds } from "../src/features/preferences/palette.ts";
import {
  defaultFonts,
  defaultTheme,
  fontSettings,
  loadFonts,
  loadSeeds,
  loadTheme,
  resolvesToDark,
  saveFonts,
  saveSeeds,
  saveTheme,
  seedSettings,
  themeSetting,
} from "../src/features/preferences/settings.ts";
test("font preferences survive refresh and reset within independent size limits", () => {
  let saved = null;
  const storage = {
    getItem: () => saved,
    setItem: (_, value) => {
      saved = value;
    },
  };
  assert.deepEqual(loadFonts(storage), defaultFonts);
  saveFonts(storage, { chat: 22, ui: 17 });
  assert.deepEqual(loadFonts(storage), { chat: 22, ui: 17 });
  assert.deepEqual(fontSettings({ chat: 100, ui: 3 }), { chat: 24, ui: 12 });
  saveFonts(storage, defaultFonts);
  assert.deepEqual(loadFonts(storage), defaultFonts);
});
test("malformed or unavailable browser storage keeps the interface usable", () => {
  assert.deepEqual(loadFonts({ getItem: () => "broken" }), defaultFonts);
  assert.deepEqual(
    loadFonts({ getItem: () => '{"chat":null,"ui":"huge"}' }),
    defaultFonts,
  );
  const blocked = {
    getItem: () => {
      throw Error();
    },
    setItem: () => {
      throw Error();
    },
  };
  assert.deepEqual(loadFonts(blocked), defaultFonts);
  assert.doesNotThrow(() => saveFonts(blocked, { chat: 20, ui: 16 }));
});
test("theme preference defaults to the system and survives refresh", () => {
  let saved = null;
  const storage = {
    getItem: () => saved,
    setItem: (_, value) => {
      saved = value;
    },
  };
  assert.equal(loadTheme(storage), defaultTheme);
  for (const choice of ["light", "dark", "system"]) {
    saveTheme(storage, choice);
    assert.equal(loadTheme(storage), choice);
  }
});
test("unusable stored themes fall back to following the system", () => {
  assert.equal(themeSetting(null), defaultTheme);
  assert.equal(themeSetting(7), defaultTheme);
  assert.equal(loadTheme({ getItem: () => "midnight" }), defaultTheme);
  const blocked = {
    getItem: () => {
      throw Error();
    },
    setItem: () => {
      throw Error();
    },
  };
  assert.equal(loadTheme(blocked), defaultTheme);
  assert.doesNotThrow(() => saveTheme(blocked, "dark"));
});
test("a pinned theme overrides the system and System follows it", () => {
  assert.equal(resolvesToDark("dark", false), true);
  assert.equal(resolvesToDark("light", true), false);
  assert.equal(resolvesToDark("system", true), true);
  assert.equal(resolvesToDark("system", false), false);
});
test("custom palette seeds survive refresh and expand on the way in", () => {
  let saved = null;
  const storage = {
    getItem: () => saved,
    setItem: (_, value) => {
      saved = value;
    },
  };
  assert.deepEqual(loadSeeds(storage), defaultSeeds);
  saveSeeds(storage, { base: "#123456", accent: "#ABCDEF" });
  assert.deepEqual(loadSeeds(storage), { base: "#123456", accent: "#abcdef" });
  saveSeeds(storage, { base: "#abc", accent: "#DeF" });
  assert.deepEqual(loadSeeds(storage), { base: "#aabbcc", accent: "#ddeeff" });
});
test("one unusable seed does not discard the other", () => {
  assert.deepEqual(seedSettings({ base: "nonsense", accent: "#112233" }), {
    base: defaultSeeds.base,
    accent: "#112233",
  });
  assert.deepEqual(loadSeeds({ getItem: () => '{"base":1,"accent":[]}' }), {
    ...defaultSeeds,
  });
  const blocked = {
    setItem: () => {
      throw Error();
    },
  };
  assert.doesNotThrow(() =>
    saveSeeds(blocked, { base: "#000000", accent: "#ffffff" }),
  );
});
