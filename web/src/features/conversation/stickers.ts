const STICKER_DIMENSION = 512;
export const stickerAccept = "image/png,image/jpeg,image/webp,image/gif";

/** Fits an image inside the sticker bound without upscaling. */
export function stickerSize(
  width: number,
  height: number,
  bound = STICKER_DIMENSION,
): { width: number; height: number } {
  if (
    !Number.isFinite(width) ||
    !Number.isFinite(height) ||
    width <= 0 ||
    height <= 0
  )
    throw new Error("Invalid image dimensions.");
  const scale = Math.min(1, bound / Math.max(width, height));
  return {
    width: Math.max(1, Math.round(width * scale)),
    height: Math.max(1, Math.round(height * scale)),
  };
}

export function formatBytes(size: number): string {
  if (size >= 1024 * 1024 && size % (1024 * 1024) === 0)
    return `${size / (1024 * 1024)} MiB`;
  if (size >= 1024) return `${Math.round(size / 1024)} KiB`;
  return `${size} bytes`;
}

function encode(canvas: HTMLCanvasElement, quality: number): Promise<Blob> {
  return new Promise((resolve, reject) =>
    canvas.toBlob(
      (blob) =>
        blob ? resolve(blob) : reject(new Error("Unable to encode sticker.")),
      "image/webp",
      quality,
    ),
  );
}

/**
 * GIFs keep their original bytes so animation survives. Other images are
 * resized and re-encoded as WebP; browsers without WebP encoding produce PNG.
 */
export async function prepareSticker(
  file: File,
  maxSize: number,
): Promise<Blob> {
  if (file.type === "image/gif") {
    if (file.size > maxSize)
      throw new Error(
        `GIF stickers are kept unchanged and must be at most ${formatBytes(maxSize)}.`,
      );
    return file;
  }
  if (!/^image\/(png|jpeg|webp)$/u.test(file.type))
    throw new Error("Choose a PNG, JPEG, WebP or GIF image.");
  const bitmap = await createImageBitmap(file).catch(() => {
    throw new Error("Unable to read this image.");
  });
  try {
    const { width, height } = stickerSize(bitmap.width, bitmap.height);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("Unable to resize sticker.");
    context.drawImage(bitmap, 0, 0, width, height);
    for (const quality of [0.85, 0.7, 0.5]) {
      const blob = await encode(canvas, quality);
      if (blob.size > 0 && blob.size <= maxSize) return blob;
    }
    throw new Error(
      `This image is still larger than ${formatBytes(maxSize)} after compression.`,
    );
  } finally {
    bitmap.close();
  }
}
