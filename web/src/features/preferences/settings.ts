import type { Fonts } from "../../api/protocol.ts";
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
export function loadFonts(storage: Pick<Storage, "getItem">) {
  try {
    return fontSettings(
      JSON.parse(storage.getItem("commonroom-fonts") || "null"),
    );
  } catch {
    return { ...defaultFonts };
  }
}
export function saveFonts(storage: Pick<Storage, "setItem">, value: Fonts) {
  try {
    storage.setItem("commonroom-fonts", JSON.stringify(fontSettings(value)));
  } catch {
    /* Font controls still work when browser storage is unavailable. */
  }
}
