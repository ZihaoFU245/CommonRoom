import type { ComponentChildren, JSX } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { errorMessage } from "../api/protocol.ts";
import { serverBaseUrl } from "../api/server.ts";

/** Remote downloads resolve authorization with fetch, which supplies Origin. */
export function MediaDownload({
  id,
  href,
  children,
  className,
}: {
  id: string;
  href: string;
  children: ComponentChildren;
  className?: string;
}) {
  const request = useRef<AbortController | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => () => request.current?.abort(), []);
  async function download() {
    const controller = new AbortController();
    request.current = controller;
    setBusy(true);
    setError("");
    try {
      const link = await api.fileLink("media", id, true, controller.signal);
      if (controller.signal.aborted) return;
      const anchor = document.createElement("a");
      anchor.href = link.url;
      anchor.download = "";
      anchor.rel = "noopener noreferrer";
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
    } catch (error) {
      if (!controller.signal.aborted) setError(errorMessage(error));
    } finally {
      if (!controller.signal.aborted) {
        request.current = null;
        setBusy(false);
      }
    }
  }
  function click(event: JSX.TargetedMouseEvent<HTMLAnchorElement>) {
    if (
      new URL(href, serverBaseUrl()).origin === new URL(document.baseURI).origin
    )
      return;
    event.preventDefault();
    if (request.current) return;
    download().catch((error: unknown) => setError(errorMessage(error)));
  }
  return (
    <>
      <a
        class={className}
        href={href}
        target="_blank"
        rel="noopener noreferrer"
        onClick={click}
        aria-busy={busy}
      >
        {children}
      </a>
      {busy && <span role="status"> Preparing download…</span>}
      {error && <span role="alert"> {error}</span>}
    </>
  );
}
