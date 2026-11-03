import type { ComponentChildren } from "preact";
import { useFileUrl } from "../hooks/useFileUrl.ts";
import { mediaDownloadUrl } from "../api/mediaFile.ts";

/** Downloads stream through the same authorized byte cache as displayed media. */
export function MediaDownload({
  href,
  children,
  className,
}: {
  href: string;
  children: ComponentChildren;
  className?: string;
}) {
  const file = useFileUrl(href);
  return (
    <>
      <a
        class={className}
        href={
          file.url ? mediaDownloadUrl(file.url, document.baseURI) : undefined
        }
        target="_blank"
        rel="noopener noreferrer"
        aria-disabled={!file.url}
        aria-busy={!file.url && !file.error}
      >
        {children}
      </a>
      {file.error && (
        <span role="alert">
          {file.error}{" "}
          <button type="button" onClick={file.retry}>
            Retry
          </button>
        </span>
      )}
    </>
  );
}
