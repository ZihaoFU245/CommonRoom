import { useState } from "preact/hooks";
import { api, ApiError } from "../api/client.ts";
import type { LibraryItem } from "../api/library.ts";
import type { CompletedMedia } from "../api/media.ts";
import { serverBaseUrl } from "../api/server.ts";
import { errorMessage } from "../api/protocol.ts";
import type { ValueRef } from "./refs.ts";

function reportFailure(error: unknown) {
  console.error("Library action failed", error);
}

/** Keep transport failures separate from selection and invalidation callbacks. */
export function useLibraryActions(
  canSelect: boolean,
  uploadBusy: boolean,
  exiting: ValueRef<boolean>,
  select: (media: CompletedMedia) => void,
  dismiss: () => void,
  removed: (id: string) => void,
) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [confirmation, setConfirmation] = useState<LibraryItem | null>(null);
  async function choose(item: LibraryItem) {
    if (busy || !canSelect || exiting.current) return;
    setBusy(true);
    setError("");
    try {
      if (!item.saved && !item.owned)
        await api.libraryChange(item.media_id, "save");
    } catch (failure) {
      setError(errorMessage(failure));
      return;
    } finally {
      setBusy(false);
    }
    select({
      ...item,
      path: new URL(`api/media/${item.media_id}/file`, serverBaseUrl())
        .pathname,
    });
    dismiss();
  }
  async function acquire(item: LibraryItem) {
    if (busy || uploadBusy || exiting.current || item.owned || !item.available)
      return;
    setBusy(true);
    setError("");
    try {
      await api.libraryChange(item.media_id, "acquire");
    } catch (failure) {
      setError(errorMessage(failure));
      return;
    } finally {
      setBusy(false);
    }
    window.dispatchEvent(
      new CustomEvent("commonroom-library-change", { detail: [item.media_id] }),
    );
  }
  async function remove(item: LibraryItem, confirmed = false) {
    if (busy || uploadBusy || exiting.current) return;
    if (item.owned && item.available && item.owner_count === 1 && !confirmed) {
      setConfirmation(item);
      return;
    }
    setBusy(true);
    setError("");
    try {
      if (item.owned)
        await api.libraryChange(item.media_id, "release", undefined, confirmed);
      await api.libraryChange(item.media_id, "remove");
      setConfirmation(null);
    } catch (failure) {
      if (failure instanceof ApiError && failure.status === 409)
        setConfirmation(item);
      else setError(errorMessage(failure));
      return;
    } finally {
      setBusy(false);
      window.dispatchEvent(
        new CustomEvent("commonroom-library-change", {
          detail: [item.media_id],
        }),
      );
    }
    removed(item.media_id);
  }
  return {
    busy,
    error,
    confirmation,
    cancelConfirmation: () => setConfirmation(null),
    choose: (item: LibraryItem) => {
      choose(item).catch(reportFailure);
    },
    acquire: (item: LibraryItem) => {
      acquire(item).catch(reportFailure);
    },
    remove: (item: LibraryItem, confirmed = false) => {
      remove(item, confirmed).catch(reportFailure);
    },
  };
}
