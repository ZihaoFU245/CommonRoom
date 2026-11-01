import { useEffect, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { serverBaseUrl } from "../api/server.ts";
import { errorMessage } from "../api/protocol.ts";

/** Keep local files on the private-cache path; remote files load directly from storage. */
export function useFileUrl(href: string | undefined, version = 0) {
  const root = new URL(serverBaseUrl());
  const url = href ? new URL(href, root) : undefined;
  const match =
    url?.origin === root.origin &&
    root.origin !== new URL(document.baseURI).origin &&
    !url.search &&
    !url.hash &&
    url.pathname.startsWith(root.pathname)
      ? /^api\/(media|avatars)\/([a-f0-9-]{36})\/file$/iu.exec(
          url.pathname.slice(root.pathname.length),
        )
      : null;
  const kind = match?.[1];
  const id = match?.[2];
  const remote = (kind === "media" || kind === "avatars") && !!id;
  const [attempt, setAttempt] = useState(0);
  const [resolved, setResolved] = useState<{
    href: string;
    version: number;
    attempt: number;
    url?: string;
    error: string;
  }>();
  useEffect(() => {
    if (!remote || !href || !id || (kind !== "media" && kind !== "avatars"))
      return;
    const controller = new AbortController();
    api.fileLink(kind, id, false, controller.signal).then(
      (link) => {
        if (!controller.signal.aborted)
          setResolved({ href, version, attempt, url: link.url, error: "" });
      },
      (error: unknown) => {
        if (!controller.signal.aborted)
          setResolved({ href, version, attempt, error: errorMessage(error) });
      },
    );
    return () => controller.abort();
  }, [remote, href, kind, id, version, attempt]);
  const current =
    resolved?.href === href &&
    resolved?.version === version &&
    resolved?.attempt === attempt
      ? resolved
      : undefined;
  return {
    url: remote ? current?.url : href,
    error: remote ? (current?.error ?? "") : "",
    retry: () => setAttempt((value) => value + 1),
  };
}
