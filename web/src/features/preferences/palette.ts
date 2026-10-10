/**
 * Derives a complete themed palette from two seed colors.
 *
 * Text and border tones are solved for a contrast target instead of being read
 * from a fixed ramp, so any seed still yields a readable interface: a mid-tone
 * text tone is found by bisecting HSL lightness against the base surface.
 */

export const paletteTokens = [
  "paper",
  "ink",
  "muted",
  "subtle",
  "line",
  "wash",
  "accent",
  "accent-hover",
  "accent-ink",
  "accent-label",
  "accent-contrast",
  "field",
  "placeholder",
  "danger",
  "online",
  "offline",
  "scrim",
  "hairline",
  "shadow-float",
  "shadow-pop",
  "shadow-dialog",
] as const;
export type PaletteToken = (typeof paletteTokens)[number];
export type Palette = Record<PaletteToken, string>;
export type Polarity = "light" | "dark";

/* Where black and white text cross over; below this a surface reads as dark. */
const DARK_BELOW = Math.sqrt(0.05 * 1.05) - 0.05;
const HEX = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i;
type Rgb = [number, number, number];

export const defaultSeeds = { base: "#181c26", accent: "#cc7d5e" };

const clamp = (value: number, low: number, high: number) =>
  Math.min(high, Math.max(low, value));

/** Normalises "#abc", "#AABBCC" or "aabbcc" to "#aabbcc"; anything else is null. */
export function parseHex(input: unknown): string | null {
  if (typeof input !== "string") return null;
  const match = HEX.exec(input.trim());
  if (!match) return null;
  const digits = match[1] ?? "";
  const full =
    digits.length === 3
      ? digits
          .split("")
          .map((digit) => digit + digit)
          .join("")
      : digits;
  return `#${full.toLowerCase()}`;
}

function rgbOf(hex: string): Rgb {
  const digits = hex.slice(1);
  return [
    parseInt(digits.slice(0, 2), 16),
    parseInt(digits.slice(2, 4), 16),
    parseInt(digits.slice(4, 6), 16),
  ];
}
function hexOf(channels: Rgb): string {
  return `#${channels
    .map((value) =>
      Math.round(clamp(value, 0, 255))
        .toString(16)
        .padStart(2, "0"),
    )
    .join("")}`;
}
function linear(value: number) {
  const scaled = value / 255;
  return scaled <= 0.04045 ? scaled / 12.92 : ((scaled + 0.055) / 1.055) ** 2.4;
}
export function relativeLuminance(hex: string): number {
  const [r, g, b] = rgbOf(hex);
  return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
}
export function contrast(first: string, second: string): number {
  const a = relativeLuminance(first);
  const b = relativeLuminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

interface Hsl {
  h: number;
  s: number;
  l: number;
}
function hslOf(hex: string): Hsl {
  const [r8, g8, b8] = rgbOf(hex);
  const r = r8 / 255;
  const g = g8 / 255;
  const b = b8 / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const span = max - min;
  if (span === 0) return { h: 0, s: 0, l };
  const s = l > 0.5 ? span / (2 - max - min) : span / (max + min);
  const raw =
    max === r
      ? (g - b) / span + (g < b ? 6 : 0)
      : max === g
        ? (b - r) / span + 2
        : (r - g) / span + 4;
  return { h: raw * 60, s, l };
}
function hslToHex(h: number, s: number, l: number): string {
  const hue = ((h % 360) + 360) % 360;
  const chroma = (1 - Math.abs(2 * l - 1)) * s;
  const second = chroma * (1 - Math.abs(((hue / 60) % 2) - 1));
  const offset = l - chroma / 2;
  const base: Rgb =
    hue < 60
      ? [chroma, second, 0]
      : hue < 120
        ? [second, chroma, 0]
        : hue < 180
          ? [0, chroma, second]
          : hue < 240
            ? [0, second, chroma]
            : hue < 300
              ? [second, 0, chroma]
              : [chroma, 0, second];
  return hexOf([
    (base[0] + offset) * 255,
    (base[1] + offset) * 255,
    (base[2] + offset) * 255,
  ]);
}

/**
 * Finds the lightness nearest the surface that reaches `target` contrast in the
 * requested direction. An unreachable target falls back to the strongest tone,
 * which is the best physics allows for a mid-tone surface.
 */
function fitContrast(
  against: string,
  target: number,
  lighter: boolean,
  h: number,
  s: number,
): string {
  const extreme = lighter ? "#ffffff" : "#000000";
  if (contrast(extreme, against) < target) return extreme;
  const surface = hslOf(against).l;
  /* One bound always holds a tone meeting the target and the other always holds
     one that misses it; which is which depends on the direction searched. */
  let low = lighter ? surface : 0;
  let high = lighter ? 1 : surface;
  for (let step = 0; step < 26; step++) {
    const mid = (low + high) / 2;
    if (contrast(hslToHex(h, s, mid), against) >= target === lighter) {
      high = mid;
    } else {
      low = mid;
    }
  }
  /* Return the bound known to pass. The midpoint sits within rounding of the
     threshold and can land on the failing side, which would otherwise collapse
     the tone all the way to plain black or white. */
  const solved = hslToHex(h, s, lighter ? high : low);
  return contrast(solved, against) >= target ? solved : extreme;
}

/** The more legible of a light and a dark tone drawn from the fill's own hue. */
function onFill(fill: string, target: number): string {
  const { h, s } = hslOf(fill);
  const saturation = Math.min(s, 0.3);
  const light = fitContrast(fill, target, true, h, saturation);
  const dark = fitContrast(fill, target, false, h, saturation);
  return contrast(light, fill) >= contrast(dark, fill) ? light : dark;
}

export interface DerivedPalette {
  polarity: Polarity;
  tokens: Palette;
}
export function derivePalette(
  baseSeed: unknown,
  accentSeed: unknown,
): DerivedPalette {
  const base = parseHex(baseSeed) ?? defaultSeeds.base;
  const accent = parseHex(accentSeed) ?? defaultSeeds.accent;
  const baseHsl = hslOf(base);
  const accentHsl = hslOf(accent);
  const dark = relativeLuminance(base) < DARK_BELOW;
  const polarity: Polarity = dark ? "dark" : "light";

  /* Surfaces keep the seed's own hue and saturation, so the whole interface
     stays in one color family; only lightness steps down or up. */
  const foreground = dark ? "#ffffff" : "#000000";
  const surface = (delta: number) => {
    /* Near the black/white crossover, a raised surface can make even the
       strongest foreground unreadable. Reduce the step until it stays safe. */
    for (let step = 0; step < 26; step++) {
      const candidate = hslToHex(
        baseHsl.h,
        baseHsl.s,
        clamp(baseHsl.l + delta / 2 ** step, 0, 1),
      );
      if (contrast(foreground, candidate) >= 4.5) return candidate;
    }
    return base;
  };

  const paper = base;
  const wash = surface(dark ? 0.055 : -0.035);
  const field = surface(dark ? 0.038 : 0.018);

  /* Choose the surface nearest the foreground, including input fields. A
     light field can be darker than a saturated paper despite its HSL step. */
  const surfaces = [paper, wash, field];
  const against = surfaces.reduce((closest, candidate) =>
    contrast(foreground, candidate) < contrast(foreground, closest)
      ? candidate
      : closest,
  );
  const solve = (target: number, lighter: boolean, h: number, s: number) =>
    fitContrast(against, target, lighter, h, s);
  const edge = (target: number) =>
    solve(target, dark, baseHsl.h, Math.min(baseHsl.s, 0.4));

  /* Text is only lightly tinted by the seed hue; legibility comes first. */
  const textSat = Math.min(baseHsl.s * 0.5, 0.12);
  const text = (target: number) => solve(target, dark, baseHsl.h, textSat);

  const line = edge(1.32);
  const hairline = edge(1.95);

  const ink = text(12.5);
  /* The incumbent light theme treats its secondary label as full-strength ink;
     dark needs a genuine second tier, so the split mirrors the built-in themes. */
  const muted = dark ? text(7) : ink;
  const subtle = text(dark ? 5.6 : 5.5);
  const placeholder = text(4.8);

  const danger = solve(4.6, dark, 8, 0.62);
  const online = solve(3.2, dark, 155, 0.55);
  const offline = solve(3, dark, baseHsl.h, 0.08);

  /* Both labels and unread counts are text and need at least 4.5:1. */
  const accentLabel = onFill(accent, 4.6);
  const accentContrast =
    contrast("#ffffff", accent) >= 4.5 ? "#ffffff" : accentLabel;
  /* Hover must move away from the label, or the fill would lose contrast with
     its own text exactly when the pointer lands on it. */
  const labelIsDark =
    relativeLuminance(accentLabel) < relativeLuminance(accent);
  const accentHover = hslToHex(
    accentHsl.h,
    accentHsl.s,
    clamp(accentHsl.l + (labelIsDark ? 0.07 : -0.07), 0, 1),
  );

  return {
    polarity,
    tokens: {
      paper,
      ink,
      muted,
      subtle,
      line,
      wash,
      accent,
      "accent-hover": accentHover,
      "accent-ink": ink,
      "accent-label": accentLabel,
      "accent-contrast": accentContrast,
      field,
      placeholder,
      danger,
      online,
      offline,
      scrim: dark ? "#000000a6" : `${ink}55`,
      hairline,
      "shadow-float": dark ? "0 6px 20px #00000059" : "0 6px 20px #00000012",
      "shadow-pop": dark ? "0 4px 15px #00000059" : "0 4px 15px #00000012",
      "shadow-dialog": dark ? "0 12px 50px #00000073" : "0 12px 50px #00000020",
    },
  };
}
