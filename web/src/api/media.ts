export interface UploadTicket {
  media_id: string;
  upload: {
    method: "PUT";
    url: string;
    headers: { "Content-Type": "application/octet-stream" };
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
    typeof upload.expires_in !== "number"
  )
    return false;
  try {
    const url = new URL(upload.url);
    return (
      ["http:", "https:"].includes(url.protocol) &&
      !url.username &&
      !url.password
    );
  } catch {
    return false;
  }
}
export function isCompletedMedia(value: unknown): value is CompletedMedia {
  return (
    record(value) &&
    typeof value.media_id === "string" &&
    typeof value.path === "string" &&
    /^\/(?:[a-z0-9_-]+\/)*api\/media\/[a-f0-9-]{36}\/file$/iu.test(
      value.path,
    ) &&
    typeof value.filename === "string" &&
    typeof value.content_type === "string" &&
    typeof value.size === "number" &&
    Number.isSafeInteger(value.size) &&
    value.size > 0
  );
}
export function mediaMarkdown(media: CompletedMedia, base: string): string {
  const url = new URL(media.path, base);
  if (url.origin !== new URL(base).origin)
    throw new Error("Invalid attachment origin.");
  const label = media.filename.replace(/[\\[\]<>\r\n]/gu, "_");
  return `${media.content_type.startsWith("image/") ? "!" : ""}[${label}](${url.href})`;
}

export function uploadFile(
  ticket: UploadTicket,
  file: File,
  signal: AbortSignal,
  progress: (percent: number) => void,
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
    xhr.timeout = 300000;
    xhr.withCredentials = false;
    xhr.setRequestHeader("Content-Type", ticket.upload.headers["Content-Type"]);
    xhr.upload.onprogress = (event) => {
      if (event.lengthComputable)
        progress(Math.round((event.loaded * 100) / event.total));
    };
    xhr.onload = () =>
      finish(
        xhr.status >= 200 && xhr.status < 300
          ? undefined
          : new Error(`Storage upload failed (${xhr.status}).`),
      );
    xhr.onerror = () =>
      finish(
        new Error(
          "Storage upload failed. Check the connection and storage CORS settings.",
        ),
      );
    xhr.ontimeout = () => finish(new Error("Storage upload timed out."));
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
