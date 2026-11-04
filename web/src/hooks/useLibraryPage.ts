import { useEffect, useState } from "preact/hooks";
import type { LibraryPage, LibraryView } from "../api/library.ts";
import { api } from "../api/client.ts";
import { errorMessage } from "../api/protocol.ts";
import { serverBaseUrl } from "../api/server.ts";
import { useEvent } from "./useEvent.ts";

// Browser-memory metadata only: media bytes retain their private-cache checks.
const cachedPages = new Map<string, { page: LibraryPage; expires: number }>();
let cacheGeneration = 0;
let observingChanges = false;
function cachedPage(key: string | undefined) {
  if (!key) return null;
  const cached = cachedPages.get(key);
  if (cached && cached.expires > Date.now()) return cached.page;
  cachedPages.delete(key);
  return null;
}

export function useLibraryPage(
  view: LibraryView,
  {
    after = "",
    enabled = true,
    cacheScope,
    retainWhileRefreshing = false,
  }: {
    after?: string;
    enabled?: boolean;
    cacheScope?: string;
    retainWhileRefreshing?: boolean;
  } = {},
) {
  const key = JSON.stringify([serverBaseUrl(), cacheScope, view, after]);
  const cacheKey = cacheScope === undefined ? undefined : key;
  const [loaded, setLoaded] = useState<{
    key: string;
    page: LibraryPage;
  } | null>(null);
  const page = loaded?.key === key ? loaded.page : cachedPage(cacheKey);
  const [error, setError] = useState("");
  const [version, setVersion] = useState(0);
  const refresh = useEvent(() => {
    if (!retainWhileRefreshing) setLoaded(null);
    setVersion((value) => value + 1);
  });
  useEffect(() => {
    // Keep invalidating shared cached pages even while the picker is unmounted.
    if (!observingChanges) {
      observingChanges = true;
      window.addEventListener("commonroom-library-change", () => {
        cacheGeneration++;
        cachedPages.clear();
      });
    }
    window.addEventListener("commonroom-library-change", refresh);
    return () =>
      window.removeEventListener("commonroom-library-change", refresh);
  }, [refresh]);
  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    const generation = cacheGeneration;
    const cached = cachedPage(cacheKey);
    setLoaded((previous) =>
      cached
        ? { key, page: cached }
        : retainWhileRefreshing && previous?.key === key
          ? previous
          : null,
    );
    setError("");
    async function load() {
      let result;
      try {
        result = await api.library(view, after, controller.signal);
      } catch (failure) {
        if (!controller.signal.aborted) setError(errorMessage(failure));
        return;
      }
      if (!controller.signal.aborted && generation === cacheGeneration) {
        if (cacheKey) {
          cachedPages.delete(cacheKey);
          cachedPages.set(cacheKey, {
            page: result,
            expires: Date.now() + 60000,
          });
          while (cachedPages.size > 24) {
            const oldest = cachedPages.keys().next().value;
            if (oldest === undefined) break;
            cachedPages.delete(oldest);
          }
        }
        setLoaded({ key, page: result });
      }
    }
    load().catch((failure: unknown) => {
      console.error("Library page update failed", failure);
    });
    return () => controller.abort();
  }, [view, after, version, enabled, key, cacheKey, retainWhileRefreshing]);
  return { page, error, refresh };
}
