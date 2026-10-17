const htmlKey = "commonroom-markdown-html";
function load(key: string): boolean {
  try {
    return localStorage.getItem(key) === "on";
  } catch {
    return false;
  }
}
function save(key: string, enabled: boolean): void {
  try {
    localStorage.setItem(key, enabled ? "on" : "off");
  } catch {
    /* Keep the session preference when storage is unavailable. */
  }
}
export const loadMarkdownHtml = () => load(htmlKey);
export const saveMarkdownHtml = (enabled: boolean) => save(htmlKey, enabled);
