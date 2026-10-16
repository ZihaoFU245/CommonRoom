const latexKey = "commonroom-markdown-latex";

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
