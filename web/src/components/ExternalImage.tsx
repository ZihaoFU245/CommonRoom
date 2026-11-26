import {
  clientMetricClock,
  observeFirstClientImage,
  recordClientDuration,
  recordClientMetric,
} from "../api/clientMetrics.ts";
import { metric } from "../api/clientMetricSchema.ts";
import { AttachmentPlaceholder } from "./AttachmentPlaceholder.tsx";
import { serverBaseUrl } from "../api/server.ts";
import { useFileUrl } from "../hooks/useFileUrl.ts";
import { useLayoutEffect, useRef, useState } from "preact/hooks";

// Layout hints only: retain real intrinsic sizes across virtual remounts without
// prefetching images or keeping decoded bodies. Bound both entries and URL keys.
const dimensions = new Map<string, { width: number; height: number }>();

/** Consent is scoped to this exact URL, never to an entire remote host. */
export function ExternalImage({ src, alt }: { src: string; alt: string }) {
  const [approved, setApproved] = useState<string | null>(null);
  const base = new URL(serverBaseUrl());
  const url = new URL(src, base);
  const file = useFileUrl(url.href, { image: true });
  const [loaded, setLoaded] = useState<{
    href: string;
    width: number;
    height: number;
  }>();
  const size = loaded?.href === url.href ? loaded : dimensions.get(url.href);
  const [failed, setFailed] = useState<string>();
  const timing = useRef<number | undefined>();
  useLayoutEffect(() => {
    timing.current =
      file.managed && file.url && !dimensions.has(url.href)
        ? clientMetricClock()
        : undefined;
  }, [file.managed, file.url, url.href]);
  useLayoutEffect(() => {
    if (file.managed && file.error)
      recordClientMetric(metric.mediaPreviewFailed);
  }, [file.managed, file.error]);
  if (
    url.origin === base.origin ||
    url.protocol === "data:" ||
    approved === src
  )
    return file.error || (failed !== undefined && failed === file.url) ? (
      <button
        type="button"
        class="attachment-retry"
        aria-label="Retry image"
        title={file.error || "Image unavailable. Click to retry."}
        onClick={() => {
          setFailed(undefined);
          file.retry();
        }}
      >
        <AttachmentPlaceholder
          filename={alt || "Image"}
          label="Image unavailable"
        />
      </button>
    ) : file.url ? (
      <img
        class="message-image"
        {...(file.crossOrigin ? { crossOrigin: file.crossOrigin } : {})}
        src={file.url}
        width={size?.width}
        height={size?.height}
        onLoad={({ currentTarget: image }) => {
          const { naturalWidth: width, naturalHeight: height } = image;
          if (!width || !height) return;
          if (file.managed) observeFirstClientImage(image);
          recordClientDuration(metric.mediaPreview, timing.current);
          timing.current = undefined;
          if (url.href.length <= 4096) {
            dimensions.delete(url.href);
            dimensions.set(url.href, { width, height });
            if (dimensions.size > 128) {
              const oldest = dimensions.keys().next().value;
              if (oldest !== undefined) dimensions.delete(oldest);
            }
          }
          setLoaded({ href: url.href, width, height });
        }}
        onError={() => {
          if (file.managed) recordClientMetric(metric.mediaPreviewFailed);
          timing.current = undefined;
          setFailed(file.url);
        }}
        alt={alt}
        loading={size ? "lazy" : "eager"}
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
