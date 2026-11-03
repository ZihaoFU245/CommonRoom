import { useEffect, useState } from "preact/hooks";
import { mediaFileUrl } from "../api/mediaFile.ts";
import { serverBaseUrl } from "../api/server.ts";
import { errorMessage } from "../api/protocol.ts";
import {
  ensureMediaCache,
  mediaCacheReady,
} from "../features/conversation/mediaController.ts";

/** Managed files use one cache pipeline, including after a hard reload. */
export function useFileUrl(href: string | undefined, version = 0) {
  const base = serverBaseUrl();
  const parsed = href ? mediaFileUrl(href, base) : null;
  const file = parsed?.base === base ? parsed : null;
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<{ key: string; error: string }>();
  const key = JSON.stringify([file?.url.href, version, attempt]);
  const managed = !!file;
  const ready = mediaCacheReady();
  useEffect(() => {
    if (!managed || ready) return;
    let mounted = true;
    ensureMediaCache().then(
      () => {
        if (mounted) setResult({ key, error: "" });
      },
      (error: unknown) => {
        if (mounted) setResult({ key, error: errorMessage(error) });
      },
    );
    return () => {
      mounted = false;
    };
  }, [managed, ready, key]);
  const error = !ready && result?.key === key ? result.error : "";
  const source = ready ? file?.url : undefined;
  if (source && (version || attempt))
    source.searchParams.set("retry", `${version}:${attempt}`);
  return {
    url: file ? source?.href : href,
    error,
    retry: () => setAttempt((value) => value + 1),
  };
}
