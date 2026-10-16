/** Only explicit network URLs and base64 raster images may fetch. */
export function safeMediaUrl(value: string): boolean {
  if (!value || value !== value.trim() || /[\u0000-\u0020\u007f]/u.test(value))
    return false;
  if (
    /^data:image\/(?:png|jpeg|gif|webp|avif|bmp);base64,(?:[a-z0-9+/]{4})*(?:[a-z0-9+/]{2}==|[a-z0-9+/]{3}=)?$/iu.test(
      value,
    )
  )
    return value.slice(value.indexOf(",") + 1).length > 0;
  if (!/^https?:\/\//iu.test(value)) return false;
  try {
    const url = new URL(value);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      !!url.hostname &&
      !value.includes("\\")
    );
  } catch {
    return false;
  }
}
