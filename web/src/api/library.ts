export interface LibraryItem {
  media_id: string;
  filename: string;
  content_type: string;
  size: number;
  available: boolean;
  owned: boolean;
  saved: boolean;
  owner_count: number;
}
export interface LibraryPage {
  items: LibraryItem[];
  next_cursor: string | null;
  usage: number;
  quota: number;
  cleanup_bytes: number;
  cleanup_objects: number;
  oldest_cleanup: number | null;
  uploads_paused: boolean;
}
export type LibraryAction = "save" | "acquire" | "remove" | "release";
export type LibraryView = "library" | "owned" | "recent";
function count(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function item(value: unknown): value is LibraryItem {
  return (
    record(value) &&
    typeof value.media_id === "string" &&
    /^[a-f0-9]{8}-(?:[a-f0-9]{4}-){3}[a-f0-9]{12}$/u.test(value.media_id) &&
    typeof value.filename === "string" &&
    typeof value.content_type === "string" &&
    count(value.size) &&
    value.size > 0 &&
    typeof value.available === "boolean" &&
    typeof value.owned === "boolean" &&
    typeof value.saved === "boolean" &&
    count(value.owner_count)
  );
}
export function isLibraryPage(value: unknown): value is LibraryPage {
  return (
    record(value) &&
    Array.isArray(value.items) &&
    value.items.length <= 100 &&
    value.items.every(item) &&
    (value.next_cursor === null || typeof value.next_cursor === "string") &&
    count(value.usage) &&
    count(value.quota) &&
    count(value.cleanup_bytes) &&
    count(value.cleanup_objects) &&
    (value.oldest_cleanup === null || count(value.oldest_cleanup)) &&
    typeof value.uploads_paused === "boolean"
  );
}
