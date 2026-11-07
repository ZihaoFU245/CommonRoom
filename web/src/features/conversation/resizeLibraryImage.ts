const maximumEdge = 1920;
const maximumPixels = 40_000_000;

/** Only positively identified static formats may lose their original bytes. */
function staticImage(bytes: Uint8Array): boolean {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const tag = (offset: number, value: string) =>
    [...value].every(
      (letter, index) => bytes[offset + index] === letter.charCodeAt(0),
    );
  if (tag(0, "\xff\xd8\xff")) return true;
  if (tag(0, "\x89PNG\r\n\x1a\n")) {
    for (let offset = 8; offset + 12 <= bytes.length;) {
      const length = view.getUint32(offset);
      if (offset + length + 12 > bytes.length) return false;
      if (tag(offset + 4, "acTL")) return false;
      if (tag(offset + 4, "IEND")) return length === 0;
      offset += length + 12;
    }
  }
  if (tag(0, "RIFF") && tag(8, "WEBP") && bytes.length >= 12) {
    const end = view.getUint32(4, true) + 8;
    if (end !== bytes.length) return false;
    let offset = 12;
    for (; offset + 8 <= end;) {
      const length = view.getUint32(offset + 4, true);
      if (offset + 8 + length > end) return false;
      if (tag(offset, "ANIM") || tag(offset, "ANMF")) return false;
      if (tag(offset, "VP8X") && (length < 1 || (bytes[offset + 8] ?? 0) & 2))
        return false;
      offset += 8 + length + (length % 2);
    }
    return offset === end;
  }
  // GIF/AVIF may animate; unknown or malformed containers stay byte-for-byte intact.
  return false;
}

function encode(canvas: HTMLCanvasElement, type: string): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, type, 0.85));
}

/** Optimize new library uploads only, before reserving their final byte size. */
export async function resizeLibraryImage(
  file: File,
  signal: AbortSignal,
): Promise<File> {
  signal.throwIfAborted();
  const bytes = new Uint8Array(await file.arrayBuffer());
  signal.throwIfAborted();
  if (!staticImage(bytes)) return file;
  const image = new Image();
  const url = URL.createObjectURL(file);
  let canvas: HTMLCanvasElement | undefined;
  try {
    await new Promise<void>((resolve, reject) => {
      const finish = (error?: Error) => {
        image.onload = null;
        image.onerror = null;
        signal.removeEventListener("abort", abort);
        if (error) reject(error);
        else resolve();
      };
      const abort = () =>
        finish(new DOMException("Upload cancelled.", "AbortError"));
      image.onload = () => finish();
      image.onerror = () => finish(new Error("Unable to read this image."));
      signal.addEventListener("abort", abort, { once: true });
      image.src = url;
    });
    signal.throwIfAborted();
    const width = image.naturalWidth;
    const height = image.naturalHeight;
    if (!width || !height || width * height > maximumPixels)
      throw new Error("Choose an image smaller than 40 megapixels.");
    const scale = Math.min(1, maximumEdge / Math.max(width, height));
    canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(width * scale));
    canvas.height = Math.max(1, Math.round(height * scale));
    const context = canvas.getContext("2d");
    if (!context) throw new Error("Unable to resize image.");
    context.imageSmoothingQuality = "high";
    context.drawImage(image, 0, 0, canvas.width, canvas.height);
    let blob = await encode(canvas, "image/webp");
    signal.throwIfAborted();
    if (blob?.type !== "image/webp") {
      const pixels = context.getImageData(
        0,
        0,
        canvas.width,
        canvas.height,
      ).data;
      let transparent = false;
      for (let offset = 3; offset < pixels.length; offset += 4) {
        if (pixels[offset] !== 255) {
          transparent = true;
          break;
        }
      }
      blob = transparent
        ? blob?.type === "image/png"
          ? blob
          : await encode(canvas, "image/png")
        : await encode(canvas, "image/jpeg");
      signal.throwIfAborted();
    }
    if (
      !blob?.size ||
      !["image/webp", "image/png", "image/jpeg"].includes(blob.type)
    )
      throw new Error("Unable to encode image.");
    if (scale === 1 && blob.size >= file.size) return file;
    const extension =
      blob.type.slice("image/".length) === "jpeg"
        ? "jpg"
        : blob.type.slice("image/".length);
    const name = file.name.replace(/\.[^./\\]+$/u, "") || "sticker";
    return new File([blob], `${name}.${extension}`, {
      type: blob.type,
      lastModified: file.lastModified,
    });
  } finally {
    image.src = "";
    URL.revokeObjectURL(url);
    if (canvas) canvas.width = canvas.height = 0;
  }
}
