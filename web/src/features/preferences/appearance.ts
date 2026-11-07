import { controlMotion } from "./motion.ts";
import { defaultSeeds, derivePalette, paletteTokens } from "./palette.ts";
import {
  defaultTheme,
  defaultFonts,
  loadFonts,
  loadSeeds,
  loadTheme,
  resolvesToDark,
  type Seeds,
  type Theme,
} from "./settings.ts";

/* Kept in step with --paper in styles/base.css so the browser chrome matches. */
const surfaces = { light: "#FCFCFC", dark: "#070708" };

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
  root.style.setProperty(
    "--control-motion-duration",
    `${controlMotion.duration}ms`,
  );
  root.style.setProperty("--control-motion-easing", controlMotion.easing);
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
    const fonts = loadFonts();
    document.documentElement.style.setProperty(
      "--chat-font-size",
      `${fonts.chat}px`,
    );
    document.documentElement.style.setProperty(
      "--ui-font-size",
      `${fonts.ui}px`,
    );
    applyAppearance(loadTheme(), systemDark, loadSeeds());
  } catch {
    document.documentElement.style.setProperty(
      "--chat-font-size",
      `${defaultFonts.chat}px`,
    );
    document.documentElement.style.setProperty(
      "--ui-font-size",
      `${defaultFonts.ui}px`,
    );
    applyAppearance(defaultTheme, systemDark, { ...defaultSeeds });
  }
}
