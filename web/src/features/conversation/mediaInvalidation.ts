import { invalidateMediaReads } from "./mediaController.ts";
import { mediaStatusCache } from "./mediaStatusCache.ts";

/** Presentation, read authority and collection refresh share one explicit boundary. */
export function invalidateMedia(
  ids?: readonly string[],
  collections = false,
): Promise<void> {
  if (ids?.length === 0) return Promise.resolve();
  mediaStatusCache.clear(ids);
  const cleared = invalidateMediaReads(ids);
  // Paginated membership/order and quota can change even outside the affected IDs.
  if (collections && typeof window !== "undefined")
    window.dispatchEvent(
      new CustomEvent("commonroom-library-change", { detail: ids }),
    );
  return cleared;
}
