const latexKey = "commonroom-markdown-latex";
const nameColorsKey = "commonroom-name-colors";
const messageBarsKey = "commonroom-message-bars";
const messageBarOpacityKey = "commonroom-message-bar-opacity";
const nameBordersKey = "commonroom-name-borders";

export function loadLatex(): boolean {
  try {
    return localStorage.getItem(latexKey) === "on";
  } catch {
    return false;
  }
}

export function saveLatex(enabled: boolean): void {
  try {
    localStorage.setItem(latexKey, enabled ? "on" : "off");
  } catch {
    /* Keep the session preference when storage is unavailable. */
  }
}

export function loadMessageBars(): boolean {
  try {
    return localStorage.getItem(messageBarsKey) === "on";
  } catch {
    return false;
  }
}

export function saveMessageBars(enabled: boolean): void {
  try {
    localStorage.setItem(messageBarsKey, enabled ? "on" : "off");
  } catch {
    /* Keep the session preference when storage is unavailable. */
  }
}

export function loadNameBorders(): boolean {
  try {
    return localStorage.getItem(nameBordersKey) === "on";
  } catch {
    return false;
  }
}

export function saveNameBorders(enabled: boolean): void {
  try {
    localStorage.setItem(nameBordersKey, enabled ? "on" : "off");
  } catch {
    /* Keep the session preference when storage is unavailable. */
  }
}

export function loadNameColors(): boolean {
  try {
    return localStorage.getItem(nameColorsKey) === "on";
  } catch {
    return false;
  }
}
export function saveNameColors(enabled: boolean): void {
  try {
    localStorage.setItem(nameColorsKey, enabled ? "on" : "off");
  } catch {
    /* Keep the session preference when storage is unavailable. */
  }
}

const defaultMessageBarOpacity = 50;
function normalizeMessageBarOpacity(value: number): number {
  return Number.isFinite(value)
    ? Math.min(100, Math.max(0, Math.round(value)))
    : defaultMessageBarOpacity;
}
export function loadMessageBarOpacity(): number {
  try {
    const value = localStorage.getItem(messageBarOpacityKey);
    return value?.trim()
      ? normalizeMessageBarOpacity(Number(value))
      : defaultMessageBarOpacity;
  } catch {
    return defaultMessageBarOpacity;
  }
}
export function saveMessageBarOpacity(value: number): void {
  try {
    localStorage.setItem(
      messageBarOpacityKey,
      String(normalizeMessageBarOpacity(value)),
    );
  } catch {
    /* Keep the session preference when storage is unavailable. */
  }
}
