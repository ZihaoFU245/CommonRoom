import { randomUuid } from "../api/randomUuid.ts";
import { useEffect, useState } from "preact/hooks";
import { mediaDownloadUrl, mediaFileUrl } from "../api/mediaFile.ts";
import { api } from "../api/client.ts";
import { httpImageCache } from "../features/conversation/httpImageCache.ts";
import { serverBaseUrl } from "../api/server.ts";
import { errorMessage } from "../api/protocol.ts";
import {
  ensureMediaCache,
  mediaCacheReady,
  directMediaRequired,
  watchMediaRead,
} from "../features/conversation/mediaController.ts";

/** Workers cache managed files; HTTP images share bounded authorized memory bytes. */
export function useFileUrl(
  href: string | undefined,
  {
    version = 0,
    download = false,
    image = false,
  }: {
    version?: number;
    download?: boolean;
    image?: boolean;
  } = {},
) {
  const base = serverBaseUrl();
  const parsed = href ? mediaFileUrl(href, base) : null;
  const file = parsed?.base === base ? parsed : null;
  const [attempt, setAttempt] = useState("");
  const [result, setResult] = useState<{
    key: string;
    error: string;
    url?: string;
  }>();
  const key = JSON.stringify([
    file?.url.href,
    version,
    attempt,
    download,
    image,
  ]);
  const managed = !!file;
  const ready = mediaCacheReady();
  const direct = directMediaRequired();
  const managedUrl = file?.url.href;
  useEffect(() => {
    if (!managedUrl) return;
    return watchMediaRead(managedUrl);
  }, [managedUrl]);
  useEffect(() => {
    if (!managedUrl || (!direct && ready)) return;
    let mounted = true;
    let objectUrl: string | undefined;
    const controller = new AbortController();
    const loading = direct
      ? image
        ? httpImageCache.load(
            (() => {
              const retryUrl = new URL(managedUrl);
              if (version || attempt)
                retryUrl.searchParams.set("retry", `${version}:${attempt}`);
              return retryUrl.href;
            })(),
            controller.signal,
          )
        : api
            .fileLink(managedUrl, controller.signal, download)
            .then((link) => link.url)
      : ensureMediaCache().then(() => undefined);
    loading.then(
      (url) => {
        if (url?.startsWith("blob:")) {
          objectUrl = url;
          if (!mounted) URL.revokeObjectURL(url);
        }
        if (mounted) setResult({ key, error: "", ...(url ? { url } : {}) });
      },
      (error: unknown) => {
        if (mounted) setResult({ key, error: errorMessage(error) });
      },
    );
    return () => {
      mounted = false;
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [managedUrl, ready, direct, download, image, key, version, attempt]);
  const current = result?.key === key ? result : undefined;
  const error = direct || !ready ? (current?.error ?? "") : "";
  const source = direct ? current?.url : ready ? file?.url.href : undefined;
  const url = source ? new URL(source) : undefined;
  if (url && !direct && (version || attempt))
    url.searchParams.set("retry", `${version}:${attempt}`);
  return {
    url: managed
      ? url && download && !direct
        ? mediaDownloadUrl(url.href, document.baseURI)
        : url?.href
      : href,
    error,
    crossOrigin: direct && managed ? ("anonymous" as const) : undefined,
    retry: () => setAttempt(randomUuid()),
  };
}
