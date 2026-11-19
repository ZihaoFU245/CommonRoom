import { AttachmentPlaceholder } from "./AttachmentPlaceholder.tsx";
import type { ComponentChildren } from "preact";
import { useContext, useEffect, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import type { MediaInfo } from "../api/media.ts";
import { LibrarySourceContext } from "./LibrarySourceContext.ts";
import { errorMessage } from "../api/protocol.ts";
import {
  MessageActionMenu,
  useMessageActionMenu,
} from "./MessageActionMenu.tsx";

/** Metadata remains readable after expiry; file bytes never do. */
export function ManagedAttachment({
  id,
  children,
}: {
  id: string;
  children: ComponentChildren;
}) {
  const [checked, setChecked] = useState<{ id: string; info?: MediaInfo }>();
  const current = checked?.id === id ? checked : undefined;
  const info = current?.info ?? api.cachedMediaStatus(id);
  const loading = !current && !info;
  const source = useContext(LibrarySourceContext);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(info?.saved === true);
  const dangling = !!info?.library && info.expires !== null;
  const ownershipCost = dangling
    ? `Acquires ownership and uses ${info.size.toLocaleString()} bytes of storage`
    : undefined;
  const [error, setError] = useState("");
  const canSave = Boolean(
    info?.library && source && !info.expired && !info.unavailable,
  );
  const actionLabel = saved
    ? dangling
      ? "Keep in collection"
      : "Saved"
    : "Add to collection";
  const stickerMenu = useMessageActionMenu(
    canSave && (!saved || dangling),
    "sticker",
  );
  useEffect(() => {
    const controller = new AbortController();
    let revision = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    function check(refresh = false) {
      if (controller.signal.aborted) return;
      const currentRevision = ++revision;
      api
        .mediaStatus(id, controller.signal, refresh)
        .then(
          (media) => {
            if (controller.signal.aborted || currentRevision !== revision)
              return;
            setSaved(media.saved === true);
            setChecked({ id, info: media });
            clearTimeout(timer);
            if (!media.expired && media.expires !== null) {
              // Recheck long lifetimes without overflowing the browser timer limit.
              timer = setTimeout(
                () => check(true),
                Math.min(
                  2_147_483_647,
                  Math.max(1000, media.expires * 1000 - Date.now() + 100),
                ),
              );
            }
          },
          () => {
            // A failed metadata read must not be mislabeled as expiry.
            if (!controller.signal.aborted && currentRevision === revision)
              setChecked((previous) =>
                previous?.id === id ? previous : { id },
              );
          },
        )
        .catch((error: unknown) => {
          console.error("Attachment status update failed", error);
        });
    }
    function resume(event: Event) {
      if (
        event instanceof CustomEvent &&
        Array.isArray(event.detail) &&
        !event.detail.includes(id)
      )
        return;
      if (document.visibilityState === "visible")
        check(event.type === "commonroom-library-change");
    }
    check();
    document.addEventListener("visibilitychange", resume);
    window.addEventListener("commonroom-library-change", resume);
    return () => {
      controller.abort();
      clearTimeout(timer);
      document.removeEventListener("visibilitychange", resume);
      window.removeEventListener("commonroom-library-change", resume);
    };
  }, [id]);
  async function save() {
    setSaving(true);
    setError("");
    try {
      await api.libraryChange(id, "save", source);
    } catch (failure) {
      setError(errorMessage(failure));
      return;
    } finally {
      setSaving(false);
    }
    setSaved(true);
    if (info) setChecked({ id, info: { ...info, saved: true, expires: null } });
    window.dispatchEvent(
      new CustomEvent("commonroom-library-change", { detail: [id] }),
    );
  }
  function saveSticker() {
    save().catch((failure: unknown) => {
      console.error("Library save update failed", failure);
    });
  }
  return (
    <span
      class="managed-attachment"
      aria-busy={loading}
      data-layout-pending={loading ? "" : undefined}
    >
      {info?.expired ? (
        <AttachmentPlaceholder
          filename={info.filename}
          label="Attachment expired"
        />
      ) : info?.unavailable ? (
        <AttachmentPlaceholder
          filename={info.filename}
          label="Media unavailable"
        />
      ) : (
        <>
          {canSave ? (
            <span class="sticker-hold-surface" {...stickerMenu.gesture}>
              {children}
              {(!saved || dangling) && (
                <button
                  type="button"
                  class="sticker-menu-trigger visually-hidden"
                  ref={stickerMenu.trigger}
                  aria-label="Sticker actions"
                  aria-haspopup="menu"
                  aria-expanded={Boolean(stickerMenu.position)}
                  onClick={(event) =>
                    stickerMenu.position
                      ? stickerMenu.close()
                      : stickerMenu.openFromTrigger(event.detail === 0)
                  }
                  onKeyDown={(event) => {
                    if (event.key !== "ArrowDown") return;
                    event.preventDefault();
                    stickerMenu.openFromTrigger(true);
                  }}
                >
                  Sticker actions
                </button>
              )}
            </span>
          ) : (
            children
          )}
          {info?.library && source && (
            <span class="library-save">
              <button
                type="button"
                disabled={saving || (saved && !dangling)}
                title={
                  ownershipCost ??
                  "Save to your collection without acquiring ownership"
                }
                onClick={saveSticker}
              >
                {actionLabel}
              </button>
              {error && <span role="status">{error}</span>}
            </span>
          )}
        </>
      )}
      {stickerMenu.position && (
        <MessageActionMenu
          position={stickerMenu.position}
          trigger={stickerMenu.trigger}
          label="Sticker actions"
          actions={[
            {
              label: actionLabel,
              accessibleLabel: actionLabel,
              description: ownershipCost,
              disabled: saving,
              run: saveSticker,
            },
          ]}
          onClose={stickerMenu.close}
        />
      )}
    </span>
  );
}
