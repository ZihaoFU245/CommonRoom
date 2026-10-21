export function avatarSource(
  value: string | undefined,
  base: string,
): string | undefined {
  if (!value) return undefined;
  if (/^\/api\/avatars\/[a-f0-9-]{36}\/file$/u.test(value))
    return new URL(value.slice(1), base).href;
  try {
    const url = new URL(value);
    if (url.protocol === "https:" && !url.username && !url.password)
      return url.href;
  } catch {
    /* Invalid profile values have no avatar. */
  }
  return undefined;
}
export function cropSquare(
  width: number,
  height: number,
  zoom: number,
  x: number,
  y: number,
) {
  if (
    ![width, height, zoom, x, y].every(Number.isFinite) ||
    width <= 0 ||
    height <= 0 ||
    zoom < 1
  )
    throw new Error("Invalid crop dimensions.");
  const size = Math.min(width, height) / zoom;
  return {
    x: ((width - size) * Math.max(0, Math.min(100, x))) / 100,
    y: ((height - size) * Math.max(0, Math.min(100, y))) / 100,
    size,
  };
}
export const AVATAR_SIZE = 256;
export const MAX_AVATAR_BYTES = 64 * 1024;

export async function encodeAvatar(source: HTMLCanvasElement): Promise<Blob> {
  const output = source.ownerDocument.createElement("canvas");
  const context = output.getContext("2d");
  if (!context) throw new Error("Unable to resize avatar.");
  // PNG fallback in browsers without WebP encoding must fit the same byte cap.
  for (const size of [AVATAR_SIZE, 128, 96]) {
    output.width = size;
    output.height = size;
    context.drawImage(source, 0, 0, size, size);
    const blob = await new Promise<Blob>((resolve, reject) =>
      output.toBlob(
        (value) =>
          value
            ? resolve(value)
            : reject(new Error("Unable to encode avatar.")),
        "image/webp",
        0.8,
      ),
    );
    if (blob.size > 0 && blob.size <= MAX_AVATAR_BYTES) return blob;
  }
  throw new Error("Compressed avatar exceeds 64 KiB.");
}
