import {
  readStorage,
  readStorageJson,
  writeStorage,
} from "../../api/browserStorage.ts";
import type { Fonts } from "../../api/protocol.ts";
import { defaultSeeds, parseHex } from "./palette.ts";
export const defaultFonts = { chat: 16, ui: 14 };
export function fontSettings(input: unknown): Fonts {
  const value =
    typeof input === "object" && input !== null
      ? (input as Record<string, unknown>)
      : {};
  const size = (key: keyof Fonts, min: number, max: number) =>
    typeof value[key] === "number" && Number.isFinite(value[key])
      ? Math.round(Math.max(min, Math.min(max, value[key])))
      : defaultFonts[key];
  return { chat: size("chat", 14, 24), ui: size("ui", 12, 18) };
}
export function loadFonts(storage?: Pick<Storage, "getItem">) {
  return fontSettings(readStorageJson("commonroom-fonts", storage));
}
export function saveFonts(storage: Pick<Storage, "setItem">, value: Fonts) {
  writeStorage(
    "commonroom-fonts",
    JSON.stringify(fontSettings(value)),
    storage,
  );
}

/** Appearance chosen in this browser: follow the system, or pin one theme. */
export type Theme = "system" | "light" | "dark" | "custom";
export const defaultTheme: Theme = "system";
export function themeSetting(input: unknown): Theme {
  return input === "light" || input === "dark" || input === "custom"
    ? input
    : defaultTheme;
}
export function loadTheme(storage?: Pick<Storage, "getItem">) {
  return themeSetting(readStorage("commonroom-theme", storage));
}
export function saveTheme(storage: Pick<Storage, "setItem">, value: Theme) {
  writeStorage("commonroom-theme", themeSetting(value), storage);
}
/** A pinned choice wins; "system" defers to the operating system preference. */
export function resolvesToDark(theme: Theme, systemDark: boolean) {
  return theme === "dark" || (theme === "system" && systemDark);
}

/** The two colors a custom palette is generated from. */
export interface Seeds {
  base: string;
  accent: string;
}
export function seedSettings(input: unknown): Seeds {
  const value =
    typeof input === "object" && input !== null
      ? (input as Record<string, unknown>)
      : {};
  return {
    base: parseHex(value["base"]) ?? defaultSeeds.base,
    accent: parseHex(value["accent"]) ?? defaultSeeds.accent,
  };
}
export function loadSeeds(storage?: Pick<Storage, "getItem">) {
  return seedSettings(readStorageJson("commonroom-palette", storage));
}
export function saveSeeds(storage: Pick<Storage, "setItem">, value: unknown) {
  writeStorage(
    "commonroom-palette",
    JSON.stringify(seedSettings(value)),
    storage,
  );
}
