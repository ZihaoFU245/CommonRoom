import assert from "node:assert/strict";
import { beforeEach, default as test } from "node:test";
import { defaultSeeds } from "../src/features/preferences/palette.ts";
import {
  loadMarkdownHtml,
  saveMarkdownHtml,
} from "../src/features/preferences/security.ts";
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
import {
  loadLatex,
  saveLatex,
  loadNameColors,
  saveNameColors,
  loadNameBorders,
  saveNameBorders,
  loadMessageBars,
  saveMessageBars,
  loadMessageBarOpacity,
  saveMessageBarOpacity,
} from "../src/features/preferences/formatting.ts";
import {
  loadMarkdown,
  saveMarkdown,
  setMarkdown,
} from "../src/features/preferences/markdown.ts";
import {
  browserCommands,
  setAlias,
} from "../src/features/conversation/aliases.ts";

let entries;
let storage;
beforeEach((t) => {
  const descriptor = Object.getOwnPropertyDescriptor(
    globalThis,
    "localStorage",
  );
  entries = new Map();
  storage = {
    getItem: (key) => entries.get(key) ?? null,
    setItem: (key, value) => entries.set(key, value),
  };
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: storage,
  });
  t.after(() => {
    if (descriptor)
      Object.defineProperty(globalThis, "localStorage", descriptor);
    else delete globalThis.localStorage;
  });
});

test("formatting preferences use their defaults and persist independently", () => {
  const preferences = [
    [loadMarkdown, saveMarkdown, true],
    [loadMarkdownHtml, saveMarkdownHtml, false],
    [loadLatex, saveLatex, false],
    [loadNameColors, saveNameColors, true],
    [loadNameBorders, saveNameBorders, false],
    [loadMessageBars, saveMessageBars, true],
  ];
  const defaults = preferences.map(([, , enabled]) => enabled);
  const current = () => preferences.map(([load]) => load());
  assert.deepEqual(current(), defaults);
  for (const [index, [, save, enabled]] of preferences.entries()) {
    save(!enabled);
    assert.deepEqual(
      current(),
      defaults.map((value, other) => (other === index ? !value : value)),
    );
    save(enabled);
    assert.deepEqual(current(), defaults);
  }
  for (const key of entries.keys()) entries.set(key, "true");
  assert.deepEqual(current(), defaults);
});

test("Markdown settings and the reserved slash command share one preference", () => {
  saveMarkdown(true);
  assert.equal(setMarkdown("/markdown off"), false);
  assert.equal(loadMarkdown(), false);
  assert.equal(setMarkdown("/markdown on"), true);
  assert.equal(loadMarkdown(), true);
  for (const input of ["/markdown", "/markdown yes", "/markdown on extra"])
    assert.throws(() => setMarkdown(input), /Usage/);
  assert.throws(() => setAlias({}, "markdown", "ls"));
  assert.ok(
    browserCommands([], {}).some((command) => command.name === "/markdown"),
  );
});

test("retired HTML opt-ins cannot enable HTML rendering or get read", () => {
  entries.set("commonroom-markdown-html-code", "on");
  const reads = [];
  storage.getItem = (key) => {
    reads.push(key);
    return entries.get(key) ?? null;
  };
  assert.equal(loadMarkdownHtml(), false);
  assert.deepEqual(reads, ["commonroom-markdown-html"]);
});

test("font preferences persist, reset and normalize invalid sizes independently", () => {
  assert.deepEqual(loadFonts(storage), defaultFonts);
  saveFonts(storage, { chat: 22, ui: 17 });
  assert.deepEqual(loadFonts(storage), { chat: 22, ui: 17 });
  assert.deepEqual(fontSettings({ chat: 100, ui: 3 }), { chat: 24, ui: 12 });
  saveFonts(storage, defaultFonts);
  assert.deepEqual(loadFonts(storage), defaultFonts);
  for (const invalid of ["broken", '{"chat":null,"ui":"huge"}']) {
    entries.set("commonroom-fonts", invalid);
    assert.deepEqual(loadFonts(storage), defaultFonts);
  }
});

test("themes persist, invalid choices follow the system and pinned choices override it", () => {
  assert.equal(loadTheme(storage), defaultTheme);
  for (const choice of ["light", "dark", "system", "custom"]) {
    saveTheme(storage, choice);
    assert.equal(loadTheme(storage), choice);
  }
  for (const invalid of [null, 7, "midnight"])
    assert.equal(themeSetting(invalid), defaultTheme);
  entries.set("commonroom-theme", "midnight");
  assert.equal(loadTheme(storage), defaultTheme);
  assert.equal(resolvesToDark("dark", false), true);
  assert.equal(resolvesToDark("light", true), false);
  assert.equal(resolvesToDark("system", true), true);
  assert.equal(resolvesToDark("system", false), false);
});

test("palette seeds persist and normalize without discarding the other seed", () => {
  assert.deepEqual(loadSeeds(storage), defaultSeeds);
  saveSeeds(storage, { base: "#123456", accent: "#ABCDEF" });
  assert.deepEqual(loadSeeds(storage), { base: "#123456", accent: "#abcdef" });
  saveSeeds(storage, { base: "#abc", accent: "#DeF" });
  assert.deepEqual(loadSeeds(storage), { base: "#aabbcc", accent: "#ddeeff" });
  assert.deepEqual(seedSettings({ base: "nonsense", accent: "#112233" }), {
    base: defaultSeeds.base,
    accent: "#112233",
  });
  entries.set("commonroom-palette", '{"base":1,"accent":[]}');
  assert.deepEqual(loadSeeds(storage), defaultSeeds);
});

test("message bar opacity persists, clamps values and defaults invalid storage to 50%", () => {
  assert.equal(loadMessageBarOpacity(), 50);
  for (const value of [0, 20, 50, 100]) {
    saveMessageBarOpacity(value);
    assert.equal(loadMessageBarOpacity(), value);
  }
  for (const [stored, expected] of [
    ["", 50],
    ["garbage", 50],
    ["NaN", 50],
    ["Infinity", 50],
    ["-10", 0],
    ["200", 100],
  ]) {
    entries.set("commonroom-message-bar-opacity", stored);
    assert.equal(loadMessageBarOpacity(), expected, stored);
  }
  saveMessageBarOpacity(NaN);
  assert.equal(loadMessageBarOpacity(), 50);
});
