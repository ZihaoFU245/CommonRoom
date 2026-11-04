/** Server directory URLs preserve API path prefixes. */
import { mediaConfigCache } from "./mediaConfigCache.ts";
import * as mediaReads from "../features/conversation/mediaController.ts";
import { mediaStatusCache } from "../features/conversation/mediaStatusCache.ts";
import { readStorage, writeStorage } from "./browserStorage.ts";
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
  } catch (error) {
    if (!(error instanceof TypeError)) throw error;
    throw new TypeError("Enter a valid server URL.");
  }
  if (
    !["https:", "http:"].includes(url.protocol) ||
    !url.hostname ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  )
    throw new TypeError(
      "Use an HTTP or HTTPS URL without credentials, a query, or a fragment.",
    );
  url.pathname = `${url.pathname.replace(/\/+$/u, "")}/`;
  return url.href;
}

function storageKey(): string {
  return `commonroom-server:${new URL(".", document.baseURI).href}`;
}

export function savedServerUrl(): string {
  const saved = readStorage(storageKey());
  try {
    if (saved) return normalizeServerUrl(saved);
  } catch (error) {
    if (!(error instanceof TypeError)) throw error;
    /* Invalid saved URLs fall back to this deployment. */
  }
  return new URL(".", document.baseURI).href;
}

export function saveServerUrl(value: string): string {
  const url = normalizeServerUrl(value || new URL(".", document.baseURI).href);
  if (!writeStorage(storageKey(), url)) {
    throw new Error(
      "Browser storage is unavailable. The server URL could not be saved.",
    );
  }
  selectedServer = url;
  mediaConfigCache.clear();
  mediaStatusCache.clear();
  mediaReads
    .invalidateMediaReads()
    .catch((error: unknown) =>
      console.error("Media access reset failed", error),
    );
  return url;
}
