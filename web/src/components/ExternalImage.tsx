import { useState } from "preact/hooks";

/** Consent is scoped to this exact URL, never to an entire remote host. */
export function ExternalImage({ src, alt }: { src: string; alt: string }) {
  const [approved, setApproved] = useState<string | null>(null);
  const url = new URL(src, document.baseURI);
  if (
    url.origin === new URL(document.baseURI).origin ||
    url.protocol === "data:" ||
    approved === src
  )
    return (
      <img
        src={src}
        alt={alt}
        loading="lazy"
        decoding="async"
        referrerPolicy="no-referrer"
      />
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
