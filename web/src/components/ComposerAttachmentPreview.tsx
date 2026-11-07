import missingAttachmentIcon from "../assets/attachment-expired.svg?no-inline";
import { useState } from "preact/hooks";
import type { Attachment } from "../hooks/useMediaUploads.ts";
import { useFileUrl } from "../hooks/useFileUrl.ts";
import { serverBaseUrl } from "../api/server.ts";

/** Uploads and library selections share the same authorized raster preview. */
export function ComposerAttachmentPreview({
  attachment,
}: {
  attachment: Attachment;
}) {
  const { media, preview } = attachment;
  const image = /^image\/(?:png|jpeg|gif|webp|avif)$/u.test(media.content_type);
  const file = useFileUrl(
    !preview && image ? new URL(media.path, serverBaseUrl()).href : undefined,
    { image: true },
  );
  const [failed, setFailed] = useState<string>();
  const source = preview ?? file.url;
  const unavailable =
    !!file.error || (source !== undefined && failed === source);
  if (!image || (!source && !unavailable)) return null;
  return (
    <img
      src={unavailable ? missingAttachmentIcon : source}
      alt={media.filename}
      {...(!preview && !unavailable && file.crossOrigin
        ? { crossOrigin: file.crossOrigin }
        : {})}
      onError={() => {
        if (source && !unavailable) setFailed(source);
      }}
    />
  );
}
