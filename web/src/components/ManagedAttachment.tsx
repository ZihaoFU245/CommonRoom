import expiredAttachmentIcon from "../assets/attachment-expired.svg?no-inline";
import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import type { MediaInfo } from "../api/media.ts";

export function ExpiredAttachment({ filename }: { filename: string }) {
  return (
    <span class="attachment-expired" role="status">
      <img
        class="attachment-expired-icon"
        src={expiredAttachmentIcon}
        width="32"
        height="32"
        alt=""
        aria-hidden="true"
        draggable={false}
      />
      <span class="attachment-expired-copy">
        <span class="attachment-expired-name" dir="auto">
          {filename}
        </span>
        <span class="attachment-expired-label">Attachment expired</span>
      </span>
    </span>
  );
}

/** Metadata remains readable after expiry; file bytes never do. */
export function ManagedAttachment({
  id,
  children,
}: {
  id: string;
  children: ComponentChildren;
}) {
  const [info, setInfo] = useState<MediaInfo | null>(null);
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function check() {
      try {
        const media = await api.mediaStatus(id, controller.signal);
        if (controller.signal.aborted) return;
        setInfo(media);
        clearTimeout(timer);
        if (!media.expired && media.expires !== null) {
          // Recheck long lifetimes without overflowing the browser timer limit.
          timer = setTimeout(
            () => {
              check().catch(() => undefined);
            },
            Math.min(
              2_147_483_647,
              Math.max(1000, media.expires * 1000 - Date.now() + 100),
            ),
          );
        }
      } catch {
        // A network or access error must not be mislabeled as expiry.
      } finally {
        if (!controller.signal.aborted) setLoading(false);
      }
    }
    function resume() {
      if (document.visibilityState === "visible")
        check().catch(() => undefined);
    }
    check().catch(() => undefined);
    document.addEventListener("visibilitychange", resume);
    return () => {
      controller.abort();
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", resume);
    };
  }, [id]);
  if (info?.expired) return <ExpiredAttachment filename={info.filename} />;
  if (loading)
    return (
      <span class="attachment-loading" role="status">
        Loading attachment…
      </span>
    );
  return children;
}
