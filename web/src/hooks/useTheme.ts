import { useEffect, useState } from "preact/hooks";
import { applyAppearance } from "../features/preferences/appearance.ts";
import { defaultSeeds } from "../features/preferences/palette.ts";
import {
  defaultTheme,
  loadSeeds,
  loadTheme,
  saveSeeds,
  saveTheme,
  type Seeds,
  type Theme,
} from "../features/preferences/settings.ts";

export function useTheme() {
  const [theme, setTheme] = useState<Theme>(() => {
    try {
      return loadTheme(localStorage);
    } catch {
      return defaultTheme;
    }
  });
  const [seeds, setSeeds] = useState<Seeds>(() => {
    try {
      return loadSeeds(localStorage);
    } catch {
      return { ...defaultSeeds };
    }
  });
  useEffect(() => {
    const system = window.matchMedia("(prefers-color-scheme: dark)");
    const sync = () => applyAppearance(theme, system.matches, seeds);
    sync();
    try {
      saveTheme(localStorage, theme);
      saveSeeds(localStorage, seeds);
    } catch {
      /* Preferences also work without storage. */
    }
    /* A custom palette carries its own polarity, so the system cannot change it. */
    if (theme !== "system") return;
    system.addEventListener("change", sync);
    return () => system.removeEventListener("change", sync);
  }, [theme, seeds]);
  return { theme, setTheme, seeds, setSeeds };
}
