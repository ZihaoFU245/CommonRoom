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
