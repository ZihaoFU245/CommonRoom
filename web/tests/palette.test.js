import assert from "node:assert/strict";
import test from "node:test";
import {
  contrast,
  defaultSeeds,
  derivePalette,
  paletteTokens,
  parseHex,
} from "../src/features/preferences/palette.ts";

test("hex seeds are normalised and malformed input is rejected", () => {
  assert.equal(parseHex("#AABBCC"), "#aabbcc");
  assert.equal(parseHex("aabbcc"), "#aabbcc");
  assert.equal(parseHex("#abc"), "#aabbcc");
  assert.equal(parseHex("  #AbC  "), "#aabbcc");
  assert.equal(parseHex("#abcd"), null);
  assert.equal(parseHex("rebeccapurple"), null);
  assert.equal(parseHex("#gggggg"), null);
  assert.equal(parseHex(7), null);
  assert.equal(parseHex(null), null);
});

test("a seed's own luminance picks the polarity", () => {
  assert.equal(derivePalette("#181c26", "#cc7d5e").polarity, "dark");
  assert.equal(derivePalette("#000000", "#cc7d5e").polarity, "dark");
  assert.equal(derivePalette("#f9f9f7", "#cc7d5e").polarity, "light");
  assert.equal(derivePalette("#ffffff", "#cc7d5e").polarity, "light");
});

test("an unusable seed falls back instead of producing broken colours", () => {
  const fallback = derivePalette("not a colour", "also not").tokens;
  assert.deepEqual(
    fallback,
    derivePalette(defaultSeeds.base, defaultSeeds.accent).tokens,
  );
  assert.equal(derivePalette(undefined, undefined).polarity, "dark");
});

test("every derived palette carries the whole token set", () => {
  const { tokens } = derivePalette("#181c26", "#cc7d5e");
  assert.deepEqual(Object.keys(tokens).sort(), [...paletteTokens].sort());
  for (const token of paletteTokens) {
    assert.match(
      tokens[token],
      /^(#[0-9a-f]{6}([0-9a-f]{2})?|0 )/,
      `${token} should be a hex colour or a shadow, got ${tokens[token]}`,
    );
  }
});

/* Any seed a person can type has to yield a readable interface: the derivation
   solves tones against these floors instead of assuming one particular ramp.
   Seeds where a floor is physically unreachable (a mid grey) are still required
   to reach the WCAG minimum, which is the promise that actually matters. */
const seeds = [
  ["the default blue-black", "#181c26", "#cc7d5e"],
  ["the light paper", "#f9f9f7", "#cc7d5e"],
  ["pure black", "#000000", "#cc7d5e"],
  ["pure white", "#ffffff", "#cc7d5e"],
  ["a navy surface with a blue accent", "#101a3a", "#4c8dff"],
  ["a forest surface with a green accent", "#0f2018", "#3ddc84"],
  ["a pale surface with a pale yellow accent", "#fdf6e3", "#ffe066"],
  ["a plum surface with a magenta accent", "#2a0d24", "#ff2fb3"],
  ["a mid grey surface", "#808080", "#cc7d5e"],
  ["a dark surface with a near-black accent", "#1b1e24", "#33261f"],
];

for (const [name, base, accent] of seeds) {
  test(`derived palette stays readable for ${name}`, () => {
    const { tokens } = derivePalette(base, accent);
    const floor = (label, foreground, background, target) => {
      const ratio = contrast(foreground, background);
      assert.ok(
        ratio >= target,
        `${label}: ${foreground} on ${background} is ${ratio.toFixed(2)}:1, needs ${target}:1`,
      );
    };
    floor("body text on paper", tokens.ink, tokens.paper, 4.5);
    floor("body text on wash", tokens.ink, tokens.wash, 4.5);
    floor("secondary label", tokens.muted, tokens.paper, 4.5);
    floor("timestamps", tokens.subtle, tokens.paper, 4.5);
    floor("timestamps on wash", tokens.subtle, tokens.wash, 4.5);
    floor("placeholder on field", tokens.placeholder, tokens.field, 4.5);
    floor("placeholder on wash", tokens.placeholder, tokens.wash, 4.5);
    floor("error text", tokens.danger, tokens.paper, 4.5);
    floor("accent label", tokens["accent-label"], tokens.accent, 4.5);
    floor(
      "accent label while hovered",
      tokens["accent-label"],
      tokens["accent-hover"],
      4.5,
    );
    floor("accent glyph", tokens["accent-contrast"], tokens.accent, 3);
    floor("online dot", tokens.online, tokens.paper, 3);
    floor("offline dot", tokens.offline, tokens.paper, 3);
    floor("border", tokens.line, tokens.paper, 1.2);
    floor("border on wash", tokens.line, tokens.wash, 1.2);
    floor("divider", tokens.hairline, tokens.paper, 1.5);
  });
}

test("hovering an accent button never loses its label", () => {
  for (const accent of [
    "#cc7d5e",
    "#4c8dff",
    "#ffe066",
    "#ff2fb3",
    "#112233",
  ]) {
    const { tokens } = derivePalette("#181c26", accent);
    assert.notEqual(tokens["accent-hover"], tokens.accent);
    assert.ok(
      contrast(tokens["accent-label"], tokens["accent-hover"]) >=
        contrast(tokens["accent-label"], tokens.accent),
      `hover must not reduce label contrast for ${accent}`,
    );
  }
});
