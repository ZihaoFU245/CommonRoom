export interface Sticker {
  id: string;
  path: string;
  content_type: string;
  size: number;
}
export interface StickerLibrary {
  enabled: boolean;
  max_size: number;
  limit: number;
  stickers: Sticker[];
}
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/u;
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function positive(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}
export function isSticker(value: unknown): value is Sticker {
  return (
    record(value) &&
    typeof value.id === "string" &&
    uuid.test(value.id) &&
    typeof value.path === "string" &&
    /^\/(?:[a-z0-9_-]+\/)*api\/stickers\/[a-f0-9-]{36}\/file$/iu.test(
      value.path,
    ) &&
    value.path.endsWith(`/${value.id}/file`) &&
    typeof value.content_type === "string" &&
    /^image\/(png|jpeg|gif|webp)$/u.test(value.content_type) &&
    positive(value.size)
  );
}
export function isStickerLibrary(value: unknown): value is StickerLibrary {
  return (
    record(value) &&
    typeof value.enabled === "boolean" &&
    positive(value.max_size) &&
    positive(value.limit) &&
    Array.isArray(value.stickers) &&
    value.stickers.every(isSticker)
  );
}
