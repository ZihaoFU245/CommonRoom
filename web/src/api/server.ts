/** Server addresses are directory URLs so API paths retain the selected prefix. */
let selectedServer: string | undefined;

export function serverBaseUrl(): string {
  selectedServer ??= savedServerUrl();
  return selectedServer;
}

export function normalizeServerUrl(value: string): string {
  const input = value.trim();
  let url: URL;
  try {
    url = new URL(input.includes("://") ? input : `https://${input}`);
  } catch {
    throw new Error("Enter a valid server URL.");
  }
  if (
    !["https:", "http:"].includes(url.protocol) ||
    !url.hostname ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  )
    throw new Error(
      "Use an HTTP or HTTPS URL without credentials, a query, or a fragment.",
    );
  url.pathname = `${url.pathname.replace(/\/+$/u, "")}/`;
  return url.href;
}

function storageKey(): string {
  return `commonroom-server:${new URL(".", document.baseURI).href}`;
}

export function savedServerUrl(): string {
  try {
    const saved = localStorage.getItem(storageKey());
    if (saved) return normalizeServerUrl(saved);
  } catch {
    /* A blocked store or invalid old value falls back to this deployment. */
  }
  return new URL(".", document.baseURI).href;
}

export function saveServerUrl(value: string): string {
  const url = normalizeServerUrl(value || new URL(".", document.baseURI).href);
  try {
    localStorage.setItem(storageKey(), url);
  } catch {
    throw new Error(
      "Browser storage is unavailable. The server URL could not be saved.",
    );
  }
  selectedServer = url;
  return url;
}
