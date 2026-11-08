import { useState } from "preact/hooks";
import type { JSX } from "preact";
import { serverBaseUrl } from "../api/server.ts";
import type { LibraryItem } from "../api/library.ts";
import { LibraryThumbnail } from "./LibraryThumbnail.tsx";
import { useSettingsMotion } from "../hooks/useSettingsMotion.ts";
import { PickerIcon } from "./PickerIcon.tsx";

// Layout hints only. Keep media authorization and bytes in the existing cache.
const ratios = new Map<string, number>();

export function LibraryStickerTile({
  item,
  disabled,
  editing,
  editable = true,
  localPreview,
  previewLoaded,
  cancelUpload,
  canSelect,
  choose,
  remove,
  acquire,
  reorder,
}: {
  item: LibraryItem;
  disabled: boolean;
  editing: boolean;
  editable?: boolean;
  localPreview?: string;
  previewLoaded?: () => void;
  cancelUpload?: () => void;
  canSelect: boolean;
  choose: () => void;
  remove: (trigger: HTMLButtonElement) => void;
  acquire: () => void;
  reorder?: {
    pointerDown: JSX.PointerEventHandler<HTMLButtonElement>;
    keyDown: JSX.KeyboardEventHandler<HTMLButtonElement>;
  };
}) {
  const actions = useSettingsMotion<HTMLDivElement>();
  const key = `${serverBaseUrl()}:${item.media_id}`;
  const [ratio, setRatio] = useState(() =>
    localPreview ? 1 : (ratios.get(key) ?? 1),
  );
  function measure(width: number, height: number) {
    if (width <= 0 || height <= 0) return;
    const next = width / height;
    if (!cancelUpload) {
      ratios.delete(key);
      ratios.set(key, next);
      if (ratios.size > 256) {
        const oldest = ratios.keys().next().value;
        if (oldest !== undefined) ratios.delete(oldest);
      }
    }
    setRatio(next);
  }
  return (
    <li
      class={`library-sticker-tile${cancelUpload ? " library-pending-upload" : ""}`}
      style={{ "--media-ratio": ratio }}
      data-library-reorder-id={reorder ? item.media_id : undefined}
    >
      <button
        class="library-preview"
        type="button"
        title={
          reorder
            ? `Drag to reorder ${item.filename}; Alt + arrow keys to move`
            : item.filename
        }
        disabled={disabled || (!canSelect && !reorder) || !item.available}
        onClick={() => {
          if (!editing) choose();
        }}
        onPointerDown={reorder?.pointerDown}
        onKeyDown={reorder?.keyDown}
        onDragStart={(event) => {
          event.preventDefault();
        }}
        draggable={false}
        aria-keyshortcuts={
          reorder
            ? "Alt+ArrowLeft Alt+ArrowRight Alt+ArrowUp Alt+ArrowDown"
            : undefined
        }
        aria-label={`Use ${item.filename}`}
      >
        {localPreview && cancelUpload ? (
          <img
            src={localPreview}
            alt=""
            onLoad={({ currentTarget }) =>
              measure(currentTarget.naturalWidth, currentTarget.naturalHeight)
            }
          />
        ) : item.available ? (
          <LibraryThumbnail
            item={item}
            {...(previewLoaded ? { onReady: previewLoaded } : {})}
            onSize={measure}
            {...(localPreview ? { preview: localPreview } : {})}
          />
        ) : (
          <span class="library-unavailable">
            <span dir="auto">{item.filename}</span>
            <span>Media unavailable</span>
          </span>
        )}
      </button>
      {cancelUpload && (
        <button
          type="button"
          class="library-upload-progress"
          aria-label={`Cancel upload of ${item.filename}`}
          onClick={cancelUpload}
        >
          <svg viewBox="0 0 32 32" role="img" aria-label="Uploading sticker">
            <circle cx="16" cy="16" r="13" />
          </svg>
          <PickerIcon name="close" />
        </button>
      )}
      {editable && (
        <div
          ref={actions}
          inert={!editing}
          data-editing={editing}
          class={`library-sticker-actions${item.owned ? " library-owned-actions" : ""}`}
          role="group"
          aria-label={`Actions for ${item.filename}`}
        >
          <button
            class="library-remove-sticker"
            type="button"
            aria-label={`Remove ${item.filename}`}
            title="Remove sticker"
            disabled={disabled}
            onClick={({ currentTarget }) => remove(currentTarget)}
          >
            <PickerIcon name="close" />
          </button>
          {!item.owned && (
            <button
              class="library-acquire-sticker"
              type="button"
              aria-label={`Acquire ${item.filename}`}
              title={
                !item.available
                  ? "Media unavailable"
                  : `Acquire ownership (${item.size.toLocaleString()} bytes of storage)`
              }
              disabled={disabled || !item.available}
              onClick={acquire}
            >
              <PickerIcon name="plus" />
            </button>
          )}
        </div>
      )}
    </li>
  );
}
