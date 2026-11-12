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
export type LibraryAction =
  "save" | "acquire" | "remove" | "release" | "release_and_remove";
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

export interface LibrarySections {
  library: LibraryPage;
  recent: LibraryPage;
}
export function isLibrarySections(value: unknown): value is LibrarySections {
  return (
    record(value) && isLibraryPage(value.library) && isLibraryPage(value.recent)
  );
}

interface PendingSections {
  controller: AbortController;
  readers: number;
  promise: Promise<LibrarySections>;
}
const pendingSections = new Map<string, PendingSections>();

/** Coalesce Recent, Collections and quota consumers without making one reader's
 * cancellation abort the other readers. Identity is part of the request key.
 */
export function sharedLibrarySections(
  key: string,
  signal: AbortSignal,
  load: (signal: AbortSignal) => Promise<LibrarySections>,
): Promise<LibrarySections> {
  if (signal.aborted)
    return Promise.reject(
      signal.reason instanceof Error
        ? signal.reason
        : new DOMException("Library request cancelled", "AbortError"),
    );
  let pending = pendingSections.get(key);
  if (!pending) {
    const controller = new AbortController();
    const entry: PendingSections = {
      controller,
      readers: 0,
      promise: Promise.resolve()
        .then(() => load(controller.signal))
        .finally(() => {
          if (pendingSections.get(key) === entry) pendingSections.delete(key);
        }),
    };
    pendingSections.set(key, entry);
    pending = entry;
  }
  const entry = pending;
  entry.readers++;
  return new Promise((resolve, reject) => {
    let released = false;
    const release = () => {
      if (released) return;
      released = true;
      signal.removeEventListener("abort", abort);
      if (--entry.readers === 0 && pendingSections.get(key) === entry) {
        pendingSections.delete(key);
        entry.controller.abort();
      }
    };
    const abort = () => {
      release();
      reject(
        signal.reason instanceof Error
          ? signal.reason
          : new DOMException("Library request cancelled", "AbortError"),
      );
    };
    signal.addEventListener("abort", abort, { once: true });
    entry.promise.then(
      (value) => {
        release();
        resolve(value);
      },
      (error: unknown) => {
        release();
        reject(
          error instanceof Error
            ? error
            : new Error("Library sections request failed", { cause: error }),
        );
      },
    );
  });
}
