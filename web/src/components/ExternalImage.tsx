import { serverBaseUrl } from "../api/server.ts";
import { useFileUrl } from "../hooks/useFileUrl.ts";
import { managedMediaLink } from "../api/media.ts";
import { useRef, useState } from "preact/hooks";

/** Consent is scoped to this exact URL, never to an entire remote host. */
export function ExternalImage({ src, alt }: { src: string; alt: string }) {
  const [approved, setApproved] = useState<string | null>(null);
  const url = new URL(src, serverBaseUrl());
  const file = useFileUrl(url.href);
  const [failed, setFailed] = useState<string>();
  const refreshed = useRef<string>();
  if (
    url.origin === new URL(serverBaseUrl()).origin ||
    url.protocol === "data:" ||
    approved === src
  )
    return file.error || (failed !== undefined && failed === file.url) ? (
      <span class="external-image">
        <button
          type="button"
          onClick={() => {
            setFailed(undefined);
            refreshed.current = undefined;
            file.retry();
          }}
        >
          Retry image
        </button>
        <span role="status">{file.error || "Unable to load image."}</span>
      </span>
    ) : file.url ? (
      <img
        src={file.url}
        onError={() => {
          if (!managedMediaLink(src, serverBaseUrl())) return;
          if (file.url !== url.href && refreshed.current !== src) {
            refreshed.current = src;
            file.retry();
          } else setFailed(file.url);
        }}
        alt={alt}
        loading="lazy"
        decoding="async"
        referrerPolicy="no-referrer"
      />
    ) : (
      <span role="status">Loading image…</span>
    );
  return (
    <span class="external-image">
      <button
        type="button"
        onClick={() => setApproved(src)}
        title={alt || undefined}
      >
        Load image from {url.host}
      </button>
      {alt && <span class="external-image-alt">{alt}</span>}
    </span>
  );
}
