import { defaultSeeds, derivePalette, paletteTokens } from "./palette.ts";
import {
  defaultTheme,
  loadSeeds,
  loadTheme,
  resolvesToDark,
  type Seeds,
  type Theme,
} from "./settings.ts";

/* Kept in step with --paper in style.css so the browser chrome matches. */
const surfaces = { light: "#F9F9F7", dark: "#181C26" };

function setBrowserColor(color: string) {
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute("content", color);
}

/** Writes a resolved appearance onto the document. */
export function applyAppearance(
  theme: Theme,
  systemDark: boolean,
  seeds: Seeds,
) {
  const root = document.documentElement;
  if (theme === "custom") {
    const { polarity, tokens } = derivePalette(seeds.base, seeds.accent);
    /* Inline properties outrank the themed blocks, so the generated palette
       replaces the built-in one without the stylesheet knowing about it. */
    for (const token of paletteTokens) {
      root.style.setProperty(`--${token}`, tokens[token]);
    }
    root.dataset["theme"] = polarity;
    setBrowserColor(tokens.paper);
    return;
  }
  for (const token of paletteTokens) root.style.removeProperty(`--${token}`);
  const dark = resolvesToDark(theme, systemDark);
  root.dataset["theme"] = dark ? "dark" : "light";
  setBrowserColor(dark ? surfaces.dark : surfaces.light);
}

/**
 * Applied once at startup, before the first render, so a saved choice is
 * already on the document when the app paints. Blocked storage still follows
 * the system preference rather than leaving the document unstyled.
 */
export function bootAppearance() {
  const systemDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  try {
    applyAppearance(
      loadTheme(localStorage),
      systemDark,
      loadSeeds(localStorage),
    );
  } catch {
    applyAppearance(defaultTheme, systemDark, { ...defaultSeeds });
  }
}
