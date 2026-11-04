import { useEffect, useMemo, useState } from "preact/hooks";
import { useAccountPreference } from "./useAccountPreference.ts";
import { applyAppearance } from "../features/preferences/appearance.ts";
import {
  loadSeeds,
  loadTheme,
  themeSetting,
  type Seeds,
  type Theme,
} from "../features/preferences/settings.ts";

export function useTheme() {
  const [local] = useState(() => ({ theme: loadTheme(), seeds: loadSeeds() }));
  const [choice, setChoice] = useAccountPreference("theme", local.theme);
  const [base, setBase] = useAccountPreference(
    "palette.base",
    local.seeds.base,
  );
  const [accent, setAccent] = useAccountPreference(
    "palette.accent",
    local.seeds.accent,
  );
  const theme = themeSetting(choice);
  useEffect(() => {
    const system = window.matchMedia("(prefers-color-scheme: dark)");
    const sync = () => applyAppearance(theme, system.matches, { base, accent });
    sync();
    if (theme !== "system") return;
    system.addEventListener("change", sync);
    return () => system.removeEventListener("change", sync);
  }, [theme, base, accent]);
  const seeds = useMemo(() => ({ base, accent }), [base, accent]);
  return {
    theme,
    setTheme: (value: Theme) => setChoice(value),
    seeds,
    setSeeds: (value: Seeds) => {
      setBase(value.base);
      setAccent(value.accent);
    },
  };
}
