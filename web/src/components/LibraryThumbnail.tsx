import { useState } from "preact/hooks";
import type { LibraryItem } from "../api/library.ts";
import { serverBaseUrl } from "../api/server.ts";
import { useFileUrl } from "../hooks/useFileUrl.ts";
export function LibraryThumbnail({
  item,
  onSize,
  preview,
  onReady,
}: {
  item: LibraryItem;
  preview?: string;
  onReady?: () => void;
  onSize: (width: number, height: number) => void;
}) {
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  const file = useFileUrl(
    new URL(`api/media/${item.media_id}/file`, serverBaseUrl()).href,
    { image: true },
  );
  return (
    <>
      {preview && !loaded && (
        <img
          src={preview}
          alt=""
          onLoad={({ currentTarget }) =>
            onSize(currentTarget.naturalWidth, currentTarget.naturalHeight)
          }
        />
      )}
      {file.url && !failed ? (
        <img
          src={file.url}
          class={preview ? undefined : "library-thumbnail"}
          style={{ opacity: !loaded ? 0 : 1 }}
          {...(file.crossOrigin ? { crossOrigin: file.crossOrigin } : {})}
          alt=""
          width="96"
          height="96"
          loading="lazy"
          onError={() => setFailed(true)}
          onLoad={({ currentTarget }) => {
            onSize(currentTarget.naturalWidth, currentTarget.naturalHeight);
            setLoaded(true);
            onReady?.();
          }}
        />
      ) : (
        !preview &&
        (file.error || failed ? (
          <span>Image unavailable</span>
        ) : (
          <span class="visually-hidden">Loading image…</span>
        ))
      )}
    </>
  );
}
