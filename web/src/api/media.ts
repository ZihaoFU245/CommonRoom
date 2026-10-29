import { requestAbort } from "./requestAbort.ts";
export interface UploadTicket {
  media_id: string;
  upload: {
    method: "PUT";
    url: string;
    headers: Record<string, string> & {
      "Content-Type": "application/octet-stream";
    };
    expires_in: number;
  };
}
export interface CompletedMedia {
  media_id: string;
  path: string;
  filename: string;
  content_type: string;
  size: number;
}
export interface MediaInfo extends CompletedMedia {
  expires: number | null;
  expired: boolean;
}
export function isMediaInfo(value: unknown): value is MediaInfo {
  return (
    isCompletedMedia(value) &&
    record(value) &&
    typeof value.expired === "boolean" &&
    (value.expires === null ||
      (typeof value.expires === "number" &&
        Number.isSafeInteger(value.expires) &&
        value.expires >= 0))
  );
}
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
export function isUploadTicket(value: unknown): value is UploadTicket {
  if (
    !record(value) ||
    typeof value.media_id !== "string" ||
    !record(value.upload)
  )
    return false;
  const upload = value.upload;
  if (
    upload.method !== "PUT" ||
    typeof upload.url !== "string" ||
    !record(upload.headers) ||
    upload.headers["Content-Type"] !== "application/octet-stream" ||
    typeof upload.expires_in !== "number" ||
    !Number.isSafeInteger(upload.expires_in) ||
    upload.expires_in <= 0 ||
    upload.expires_in > 604800
  )
    return false;
  if (
    Object.entries(upload.headers).some(
      ([key, value]) =>
        typeof value !== "string" ||
        (key !== "Content-Type" &&
          (!/^x-amz-meta-client-(filename|content-type|label|client)$/u.test(
            key,
          ) ||
            value.length < 1 ||
            value.length > 256 ||
            /[^\x20-\x7e]/u.test(value))),
    )
  )
    return false;
  try {
    const url = new URL(upload.url);
    return (
      (url.protocol === "https:" ||
        (url.protocol === "http:" &&
          (url.hostname === "localhost" ||
            url.hostname.endsWith(".localhost") ||
            url.hostname === "[::1]" ||
            /^127\.\d+\.\d+\.\d+$/u.test(url.hostname)))) &&
      !url.username &&
      !url.password
    );
  } catch {
    return false;
  }
}
export function mediaPath(path: unknown): { id: string; path: string } | null {
  if (typeof path !== "string") return null;
  const match =
    /^\/(?:[a-z0-9_-]+\/)*api\/media\/([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})\/file$/iu.exec(
      path,
    );
  return match?.[1] ? { id: match[1], path } : null;
}
export function isCompletedMedia(value: unknown): value is CompletedMedia {
  return (
    record(value) &&
    typeof value.media_id === "string" &&
    typeof value.path === "string" &&
    mediaPath(value.path)?.id === value.media_id &&
    typeof value.filename === "string" &&
    typeof value.content_type === "string" &&
    typeof value.size === "number" &&
    Number.isSafeInteger(value.size) &&
    value.size > 0
  );
}
export function mediaMarkdown(
  media: CompletedMedia,
  base: string,
): string | null {
  let url: URL;
  try {
    url = new URL(media.path, base);
    if (
      url.origin !== new URL(base).origin ||
      mediaPath(media.path)?.id !== media.media_id
    )
      return null;
  } catch {
    return null;
  }
  const label = media.filename.replace(/[\\[\]<>\r\n]/gu, "_");
  const kind = attachmentKind(media.content_type);
  const image = media.content_type.startsWith("image/") && kind !== "svg";
  return `${image ? "!" : ""}[${label}](${url.pathname}${kind ? ` "media:${kind}"` : ""})`;
}

export function uploadFile(
  ticket: UploadTicket,
  file: File,
  signal: AbortSignal,
  progress?: (percent: number) => void,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    const abort = () => xhr.abort();
    const finish = (error?: Error) => {
      signal.removeEventListener("abort", abort);
      if (error) reject(error);
      else resolve();
    };
    xhr.open("PUT", ticket.upload.url);
    // URL expiry limits when PUT may start, not the transfer's duration.
    xhr.timeout = 15 * 60 * 1000;
    xhr.withCredentials = false;
    for (const [name, value] of Object.entries(ticket.upload.headers))
      xhr.setRequestHeader(name, value);
    let previousPercent = -1;
    let previousTime = 0;
    if (progress)
      xhr.upload.onprogress = (event) => {
        if (!event.lengthComputable || event.total <= 0) return;
        const percent = Math.round((event.loaded * 100) / event.total);
        const time = performance.now();
        if (
          percent === previousPercent ||
          (previousPercent >= 0 && percent < 100 && time - previousTime < 100)
        )
          return;
        previousPercent = percent;
        previousTime = time;
        progress(percent);
      };
    xhr.onload = () =>
      finish(
        xhr.status >= 200 && xhr.status < 300
          ? undefined
          : new Error(
              xhr.status === 413
                ? "This file is too large. Choose a smaller file."
                : "Couldn't upload this file. Please try again.",
            ),
      );
    xhr.onerror = () =>
      finish(
        new Error(
          "Couldn't upload this file. Check your connection and try again.",
        ),
      );
    xhr.ontimeout = () =>
      finish(new Error("The upload took too long. Please try again."));
    xhr.onabort = () =>
      finish(new DOMException("Upload cancelled.", "AbortError"));
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted)
      finish(new DOMException("Upload cancelled.", "AbortError"));
    else xhr.send(file);
  });
}

export function isMediaConfig(
  value: unknown,
): value is { enabled: boolean; max_size: number } {
  return (
    record(value) &&
    typeof value.enabled === "boolean" &&
    typeof value.max_size === "number" &&
    Number.isSafeInteger(value.max_size) &&
    value.max_size > 0
  );
}

export type AttachmentKind = "audio" | "video" | "html" | "svg";
export function attachmentKind(type: string): AttachmentKind | null {
  if (type.startsWith("audio/")) return "audio";
  if (type.startsWith("video/")) return "video";
  if (type === "text/html") return "html";
  if (type === "image/svg+xml") return "svg";
  return null;
}
// A Markdown title is a display hint only; verify server metadata before loading.
export function attachmentLink(href: string, title: string, base: string) {
  const hint = title.startsWith("media:") ? title.slice(6) : "";
  const kind: AttachmentKind | null =
    hint === "audio" || hint === "video" || hint === "html" || hint === "svg"
      ? hint
      : null;
  if (!kind) return null;
  const parsed = managedMediaLink(href, base);
  return parsed ? { ...parsed, kind } : null;
}

/** Only canonical attachment URLs belonging to this application. */
export function managedMediaLink(href: string, base: string) {
  try {
    const url = new URL(href, base);
    const root = new URL(base);
    const parsed = mediaPath(url.pathname);
    return parsed &&
      url.origin === root.origin &&
      !url.username &&
      !url.password &&
      !url.search &&
      !url.hash &&
      url.pathname === new URL(`api/media/${parsed.id}/file`, root).pathname
      ? parsed
      : null;
  } catch {
    return null;
  }
}

export const previewLimit = 1024 * 1024;
// Read a bounded document only after a click; original bytes stay in S3.
export async function documentSource(
  path: string,
  signal: AbortSignal,
): Promise<string> {
  const cancellation = requestAbort(15000, signal);
  try {
    const response = await fetch(path, {
      credentials: "same-origin",
      cache: "no-store",
      signal: cancellation.signal,
    });
    if (!response.ok || !response.body)
      throw new Error("Unable to load preview.");
    const reader = response.body.getReader();
    const decoder = new TextDecoder("utf-8", { fatal: true });
    let size = 0;
    let source = "";
    try {
      for (;;) {
        const chunk = await reader.read();
        if (chunk.done) break;
        size += chunk.value.byteLength;
        if (size > previewLimit)
          throw new Error(
            "Preview is limited to 1 MiB. Download this file instead.",
          );
        source += decoder.decode(chunk.value, { stream: true });
      }
      return source + decoder.decode();
    } finally {
      await reader.cancel();
    }
  } finally {
    cancellation.dispose();
  }
}

// Keep complete Unicode code points and percent escapes within S3's hint limit.
// The 256-character budget applies to the encoded ASCII metadata header,
// so non-ASCII filenames intentionally consume more of the wire budget.
export function filenameHint(filename: string): string {
  let result = "";
  for (const point of filename) {
    const code = point.codePointAt(0) ?? 0;
    const encoded = encodeURIComponent(
      code >= 0xd800 && code <= 0xdfff ? "\ufffd" : point,
    );
    if (result.length + encoded.length > 256) break;
    result += encoded;
  }
  return result;
}
