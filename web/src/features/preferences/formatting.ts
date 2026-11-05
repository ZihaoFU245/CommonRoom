import { readStorage, writeStorage } from "../../api/browserStorage.ts";
const latexKey = "commonroom-markdown-latex";
const nameColorsKey = "commonroom-name-colors";
const messageBarsKey = "commonroom-message-bars";
const messageBarOpacityKey = "commonroom-message-bar-opacity";
const nameBordersKey = "commonroom-name-borders";

export function loadLatex(): boolean {
  return readStorage(latexKey) === "on";
}

export function saveLatex(enabled: boolean): void {
  writeStorage(latexKey, enabled ? "on" : "off");
}

export function loadMessageBars(): boolean {
  return readStorage(messageBarsKey) !== "off";
}

export function saveMessageBars(enabled: boolean): void {
  writeStorage(messageBarsKey, enabled ? "on" : "off");
}

export function loadNameBorders(): boolean {
  return readStorage(nameBordersKey) === "on";
}

export function saveNameBorders(enabled: boolean): void {
  writeStorage(nameBordersKey, enabled ? "on" : "off");
}

export function loadNameColors(): boolean {
  return readStorage(nameColorsKey) !== "off";
}
export function saveNameColors(enabled: boolean): void {
  writeStorage(nameColorsKey, enabled ? "on" : "off");
}

const defaultMessageBarOpacity = 50;
function normalizeMessageBarOpacity(value: number): number {
  return Number.isFinite(value)
    ? Math.min(100, Math.max(0, Math.round(value)))
    : defaultMessageBarOpacity;
}
export function loadMessageBarOpacity(): number {
  const value = readStorage(messageBarOpacityKey);
  return value?.trim()
    ? normalizeMessageBarOpacity(Number(value))
    : defaultMessageBarOpacity;
}
export function saveMessageBarOpacity(value: number): void {
  writeStorage(messageBarOpacityKey, String(normalizeMessageBarOpacity(value)));
}
