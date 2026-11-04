import type { RefObject } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import type { LibraryItem } from "../api/library.ts";
import type { CompletedMedia } from "../api/media.ts";
import { serverBaseUrl } from "../api/server.ts";
import { useMediaUploads } from "../hooks/useMediaUploads.ts";
import { useLibraryPage } from "../hooks/useLibraryPage.ts";
import { useLibraryActions } from "../hooks/useLibraryActions.ts";
import { useLibraryPopover } from "../hooks/useLibraryPopover.ts";
import { LibraryStickerTile } from "./LibraryStickerTile.tsx";
import { readStorage, writeStorage } from "../api/browserStorage.ts";
import {
  matchingEmoji,
  recentEmoji,
  rememberEmoji,
} from "../features/conversation/emojis.ts";
import type { AutoAnimationPlugin } from "@formkit/auto-animate";
import { controlMotion } from "../features/preferences/motion.ts";
import { useSettingsMotion } from "../hooks/useSettingsMotion.ts";
import { useLibraryMotion } from "../hooks/useLibraryMotion.ts";
import { PickerIcon } from "./PickerIcon.tsx";

// Panel changes fade without resizing or moving the independent preview layout.
const panelMotion: AutoAnimationPlugin = (element, action) => {
  if (action === "remove") element.setAttribute("inert", "");
  return new KeyframeEffect(
    element,
    [
      { opacity: action === "add" ? 0 : 1 },
      { opacity: action === "remove" ? 0 : 1 },
    ],
    controlMotion,
  );
};

function EmojiGrid({
  items,
  label,
  disabled,
  select,
}: {
  items: ReturnType<typeof matchingEmoji>;
  label: string;
  disabled: boolean;
  select: (emoji: string) => void;
}) {
  return (
    <div class="emoji-grid" aria-label={label}>
      {items.map(([, emoji, name]) => (
        <button
          type="button"
          key={emoji}
          title={name}
          aria-label={name}
          disabled={disabled}
          onClick={() => select(emoji)}
        >
          {emoji}
        </button>
      ))}
    </div>
  );
}
export function LibraryPicker({
  canSelect,
  canEmoji = true,
  select,
  selectEmoji,
  close,
  anchor,
  closing = false,
  account = "",
}: {
  canSelect: boolean;
  canEmoji?: boolean;
  select: (media: CompletedMedia) => void;
  selectEmoji?: (emoji: string) => void;
  close: () => void;
  anchor?: RefObject<HTMLButtonElement>;
  closing?: boolean;
  account?: string;
}) {
  const { dialog, exiting, dismiss } = useLibraryPopover(
    anchor,
    closing,
    close,
  );
  const [editing, setEditing] = useState(false);
  const panels = useSettingsMotion<HTMLDivElement>(panelMotion);
  const confirmationHost = useSettingsMotion<HTMLDivElement>(panelMotion);
  const confirmationReturnFocus = useRef<HTMLButtonElement | null>(null);
  const confirmationWasOpen = useRef(false);
  const confirmationPanel = useRef<HTMLDivElement>(null);
  const modeKey = `commonroom-picker-mode:${serverBaseUrl()}:${account}`;
  const [mode, setMode] = useState<"emoji" | "stickers">(() =>
    readStorage(modeKey) === "stickers" ? "stickers" : "emoji",
  );
  useEffect(() => {
    writeStorage(modeKey, mode);
  }, [modeKey, mode]);
  const content = useLibraryMotion(mode);
  const uploadKeys = useRef(
    new Map<string, { key: string; preview?: string }>(),
  );
  const [cursors, setCursors] = useState({
    recent: "",
    library: "",
  });
  const recentKey = `commonroom-emoji:${serverBaseUrl()}:${account}`;
  const [recent, setRecent] = useState(() => recentEmoji(recentKey));
  const uploads = useMediaUploads("@library");
  const [pendingUpload, setPendingUpload] = useState<{
    filename: string;
    preview: string;
  } | null>(null);
  const [completedUploads, setCompletedUploads] = useState<LibraryItem[]>([]);
  useEffect(() => {
    const keys = uploadKeys.current;
    return () => {
      for (const entry of keys.values()) {
        if (entry.preview) URL.revokeObjectURL(entry.preview);
      }
    };
  }, []);
  const preview = pendingUpload?.preview;
  useEffect(
    () => () => {
      if (
        preview &&
        !Array.from(uploadKeys.current.values()).some(
          (entry) => entry.key === preview,
        )
      )
        URL.revokeObjectURL(preview);
    },
    [preview],
  );
  async function upload(file: File) {
    const pending = {
      filename: file.name,
      preview: URL.createObjectURL(file),
    };
    setPendingUpload(pending);
    await uploads.start(file);
  }

  const {
    busy,
    error,
    confirmation,
    cancelConfirmation,
    choose,
    acquire,
    remove,
  } = useLibraryActions(
    canSelect,
    uploads.busy,
    exiting,
    select,
    dismiss,
    (id) => {
      confirmationReturnFocus.current = null;
      setCompletedUploads((items) =>
        items.filter((item) => item.media_id !== id),
      );
    },
  );
  useLayoutEffect(() => {
    if (confirmation) {
      if (!confirmationWasOpen.current) confirmationPanel.current?.focus();
      confirmationWasOpen.current = true;
    } else if (confirmationWasOpen.current && !busy) {
      const trigger = confirmationReturnFocus.current;
      const target =
        trigger?.isConnected && !trigger.disabled && !trigger.closest("[inert]")
          ? trigger
          : content.current?.querySelector<HTMLButtonElement>(".library-edit");
      target?.focus({ preventScroll: true });
      confirmationWasOpen.current = false;
      confirmationReturnFocus.current = null;
    }
  }, [confirmation, busy, content]);
  const enabled = mode === "stickers";
  const recentStickers = useLibraryPage("recent", {
    enabled,
    cacheScope: account,
    after: cursors.recent,
    retainWhileRefreshing: true,
  });
  const collection = useLibraryPage("library", {
    enabled,
    cacheScope: account,
    after: cursors.library,
    retainWhileRefreshing: true,
  });
  const sections = [
    { view: "recent", title: "Recent", result: recentStickers },
    { view: "library", title: "Collections", result: collection },
  ] as const;
  const loadingPage = sections.some(
    ({ result }) => !result.page && !result.error,
  );
  const [stickersLoaded, setStickersLoaded] = useState(false);
  useEffect(() => {
    if (enabled && !loadingPage) setStickersLoaded(true);
  }, [enabled, loadingPage]);
  const loadingStickers = !stickersLoaded && loadingPage;
  const uploadsPaused = sections.some(
    ({ result }) => result.page?.uploads_paused,
  );
  const { attachment: uploaded, published } = uploads;
  const refreshRecent = recentStickers.refresh;
  const refreshCollection = collection.refresh;
  useEffect(() => {
    if (!uploaded) return;
    setCompletedUploads((items) => [
      ...items,
      {
        ...uploaded.media,
        available: true,
        owned: true,
        saved: true,
        owner_count: 1,
      },
    ]);
    setPendingUpload(null);
    if (preview)
      uploadKeys.current.set(uploaded.media.media_id, {
        key: preview,
        preview,
      });
    published(uploaded.media.media_id);
    setCursors({ recent: "", library: "" });
    refreshRecent();
    refreshCollection();
  }, [uploaded, published, refreshRecent, refreshCollection, preview]);
  useEffect(() => {
    const ids = new Set(collection.page?.items.map((item) => item.media_id));
    if (completedUploads.some((item) => ids.has(item.media_id)))
      setCompletedUploads((items) =>
        items.filter((item) => !ids.has(item.media_id)),
      );
  }, [completedUploads, collection.page]);
  useEffect(() => {
    if (pendingUpload && !uploads.busy && !uploaded) setPendingUpload(null);
  }, [pendingUpload, uploads.busy, uploaded]);
  function pickEmoji(emoji: string) {
    if (exiting.current) return;
    selectEmoji?.(emoji);
    setRecent(rememberEmoji(recentKey, emoji, recent));
  }
  return (
    <dialog
      ref={dialog}
      popover="manual"
      tabIndex={-1}
      class="library-picker"
      aria-labelledby="library-title"
      onCancel={(event) => {
        event.preventDefault();
        dismiss();
      }}
    >
      <h2 id="library-title" class="visually-hidden">
        Emoji and sticker picker
      </h2>
      <div class="library-content-motion" ref={panels}>
        <div
          ref={content}
          class="library-content"
          key={mode}
          inert={confirmation !== null}
        >
          {mode === "emoji" ? (
            <>
              {recent.length > 0 && (
                <>
                  <h3 class="library-section-title">Recent</h3>
                  <EmojiGrid
                    items={matchingEmoji(0, "", recent)}
                    label="Recent emoji"
                    disabled={!canEmoji}
                    select={pickEmoji}
                  />
                </>
              )}
              <h3 class="library-section-title">Smileys &amp; people</h3>
              <EmojiGrid
                items={matchingEmoji(1, "", [])}
                label="Smileys & people"
                disabled={!canEmoji}
                select={pickEmoji}
              />
            </>
          ) : (
            <>
              {!confirmation && (error || uploads.error) && (
                <p role="status">{error || uploads.error}</p>
              )}
              {uploadsPaused && (
                <p role="status">
                  New sticker uploads are paused while storage cleanup catches
                  up.
                </p>
              )}
              {loadingStickers && (
                <p class="library-section-status" role="status">
                  Loading stickers…
                </p>
              )}
              {!loadingStickers &&
                sections.map(
                  ({ view, title, result: { page, error: sectionError } }) =>
                    (view === "library" ||
                      !page ||
                      page.items.length > 0 ||
                      sectionError ||
                      cursors[view] ||
                      page.next_cursor) && (
                      <section key={view} aria-label={title}>
                        <div class="library-section-heading">
                          <h3 class="library-section-title">{title}</h3>
                          {view === "library" && (
                            <button
                              type="button"
                              class="library-edit"
                              aria-pressed={editing}
                              onClick={() => setEditing((value) => !value)}
                            >
                              {editing ? "Done" : "Edit"}
                            </button>
                          )}
                        </div>
                        {sectionError && <p role="status">{sectionError}</p>}
                        {!page &&
                          !sectionError &&
                          sections.some(
                            ({ result }) => result.page || result.error,
                          ) && (
                            <p class="library-section-status" role="status">
                              Loading…
                            </p>
                          )}
                        <ul
                          class="library-grid"
                          aria-label={`${title} stickers`}
                        >
                          {[
                            ...[
                              ...(page?.items ?? []),
                              ...(view === "library"
                                ? completedUploads.filter(
                                    (uploaded) =>
                                      !page?.items.some(
                                        (item) =>
                                          item.media_id === uploaded.media_id,
                                      ),
                                  )
                                : []),
                            ].map((item) => {
                              const upload = uploadKeys.current.get(
                                item.media_id,
                              );
                              return (
                                <LibraryStickerTile
                                  key={upload?.key ?? item.media_id}
                                  item={item}
                                  {...(upload?.preview
                                    ? { localPreview: upload.preview }
                                    : {})}
                                  previewLoaded={() => {
                                    if (upload?.preview) {
                                      URL.revokeObjectURL(upload.preview);
                                      uploadKeys.current.set(item.media_id, {
                                        key: upload.key,
                                      });
                                    }
                                  }}
                                  editing={editing}
                                  disabled={
                                    busy ||
                                    uploads.busy ||
                                    confirmation !== null
                                  }
                                  canSelect={canSelect}
                                  choose={() => {
                                    choose(item);
                                  }}
                                  remove={(trigger) => {
                                    confirmationReturnFocus.current = trigger;
                                    remove(item);
                                  }}
                                  acquire={() => {
                                    acquire(item);
                                  }}
                                />
                              );
                            }),
                            view === "library" && pendingUpload && (
                              <LibraryStickerTile
                                key={pendingUpload.preview}
                                item={{
                                  media_id: "",
                                  filename: pendingUpload.filename,
                                  content_type: "",
                                  size: 0,
                                  available: true,
                                  owned: true,
                                  saved: true,
                                  owner_count: 1,
                                }}
                                localPreview={pendingUpload.preview}
                                cancelUpload={() => {
                                  uploads.cancel();
                                  setPendingUpload(null);
                                }}
                                editing={false}
                                disabled={true}
                                canSelect={false}
                                choose={() => undefined}
                                remove={() => undefined}
                                acquire={() => undefined}
                              />
                            ),
                            view === "library" && (
                              <li key="add-sticker" class="library-upload-tile">
                                <label
                                  class="library-add-sticker"
                                  title="Add sticker"
                                >
                                  <PickerIcon name="plus" />
                                  <span class="visually-hidden">
                                    Add sticker
                                  </span>
                                  <input
                                    type="file"
                                    aria-label="Upload sticker"
                                    accept="image/png,image/jpeg,image/gif,image/webp,image/avif"
                                    disabled={
                                      busy ||
                                      uploads.busy ||
                                      !uploads.enabled ||
                                      uploadsPaused ||
                                      confirmation !== null
                                    }
                                    onChange={(event) => {
                                      const file =
                                        event.currentTarget.files?.[0];
                                      event.currentTarget.value = "";
                                      if (file)
                                        upload(file).catch((failure: unknown) =>
                                          console.error(
                                            "Library upload failed",
                                            failure,
                                          ),
                                        );
                                    }}
                                  />
                                </label>
                              </li>
                            ),
                          ]}
                        </ul>
                        <div class="library-pagination">
                          {cursors[view] && (
                            <button
                              type="button"
                              onClick={() => {
                                if (view === "library") setCompletedUploads([]);
                                setCursors((value) => ({
                                  ...value,
                                  [view]: "",
                                }));
                              }}
                            >
                              First page
                            </button>
                          )}
                          {page?.next_cursor && (
                            <button
                              type="button"
                              onClick={() => {
                                if (view === "library") setCompletedUploads([]);
                                setCursors((value) => ({
                                  ...value,
                                  [view]: page.next_cursor ?? "",
                                }));
                              }}
                            >
                              Next page
                            </button>
                          )}
                        </div>
                      </section>
                    ),
                )}
            </>
          )}
        </div>
      </div>
      <div
        class="library-confirmation-host"
        ref={confirmationHost}
        inert={!confirmation}
      >
        {confirmation && (
          <div class="library-confirmation-layer">
            <div
              class="library-confirmation"
              role="alertdialog"
              aria-modal="true"
              aria-labelledby="library-confirmation-title"
              aria-describedby="library-confirmation-description"
              onKeyDown={(event) => {
                if (event.key !== "Tab") return;
                const buttons = Array.from(
                  event.currentTarget.querySelectorAll<HTMLButtonElement>(
                    "button:not(:disabled)",
                  ),
                );
                const index = buttons.findIndex(
                  (button) => button === document.activeElement,
                );
                const next =
                  buttons[
                    event.shiftKey
                      ? index <= 0
                        ? buttons.length - 1
                        : index - 1
                      : (index + 1) % buttons.length
                  ];
                event.preventDefault();
                next?.focus();
              }}
              ref={confirmationPanel}
              tabIndex={-1}
            >
              <h3 id="library-confirmation-title">
                Remove “{confirmation.filename}”?
              </h3>
              <p id="library-confirmation-description">
                This permanently removes it for everyone, including existing
                messages.
              </p>
              {error && <p role="alert">{error}</p>}
              <div class="library-confirmation-actions">
                <button
                  class="library-confirmation-remove"
                  type="button"
                  disabled={busy}
                  onClick={() => {
                    remove(confirmation, true);
                  }}
                >
                  Remove permanently
                </button>
                <button
                  class="library-confirmation-keep"
                  type="button"
                  disabled={busy}
                  onClick={() => cancelConfirmation()}
                >
                  Keep sticker
                </button>
              </div>
            </div>
          </div>
        )}
      </div>
      <nav
        class="library-modes"
        aria-label="Picker type"
        inert={confirmation !== null}
      >
        <button
          type="button"
          aria-label="Emojis"
          title="Emojis"
          aria-pressed={mode === "emoji"}
          onClick={() => {
            cancelConfirmation();
            setMode("emoji");
          }}
        >
          <PickerIcon name="emoji" />
        </button>
        <button
          type="button"
          aria-label="Stickers"
          title="Stickers"
          aria-pressed={mode === "stickers"}
          onClick={() => {
            setMode("stickers");
          }}
        >
          <PickerIcon name="sticker" />
        </button>
      </nav>
    </dialog>
  );
}
