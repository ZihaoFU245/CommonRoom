import { useEffect, useRef, useState } from "preact/hooks";
import { ApiError, api } from "../api/client.ts";
import { uploadFile } from "../api/media.ts";
import { errorMessage } from "../api/protocol.ts";
import type { Sticker, StickerLibrary } from "../api/stickers.ts";
import {
  DEFAULT_EMOJI_CATEGORY,
  EMOJI_CATEGORIES,
  type EmojiCategoryId,
} from "../features/conversation/emojiCategories.ts";
import {
  formatBytes,
  libraryHasDigest,
  prepareSticker,
  resolveLibraryDigests,
  stickerAccept,
  stickerDigest,
} from "../features/conversation/stickers.ts";

const DUPLICATE_STICKER = "This sticker is already in your library.";

type Mode = "emoji" | "sticker";

interface Props {
  disabled: boolean;
  pick: (sticker: Sticker) => void;
  insertEmoji: (emoji: string) => void;
  close: () => void;
}

export function StickerPicker({ disabled, pick, insertEmoji, close }: Props) {
  const [library, setLibrary] = useState<StickerLibrary | null>(null);
  const [status, setStatus] = useState("Loading stickers…");
  const [error, setError] = useState("");
  const [uploading, setUploading] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [duplicateNotice, setDuplicateNotice] = useState(false);
  const [mode, setMode] = useState<Mode>("sticker");
  const [emojiCategory, setEmojiCategory] = useState<EmojiCategoryId>(
    DEFAULT_EMOJI_CATEGORY,
  );
  const panel = useRef<HTMLDivElement>(null);
  const upload = useRef<AbortController | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const emojiGrid = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const request = new AbortController();
    api
      .stickers(request.signal)
      .then((value) => {
        if (request.signal.aborted) return;
        setLibrary(value);
        setStatus("");
      })
      .catch((failure: unknown) => {
        if (request.signal.aborted) return;
        setStatus("");
        setError(errorMessage(failure));
      });
    return () => {
      request.abort();
      upload.current?.abort();
    };
  }, []);
  useEffect(() => {
    function dismiss(event: PointerEvent) {
      const target = event.target;
      if (
        target instanceof Element &&
        target.closest(
          ".sticker-delete-confirm, .sticker-delete, .sticker-dock",
        )
      )
        return;
      if (duplicateNotice) {
        setDuplicateNotice(false);
        return;
      }
      if (pendingDelete) {
        setPendingDelete(null);
        return;
      }
      if (
        target instanceof Node &&
        !panel.current?.contains(target) &&
        !(target instanceof Element && target.closest(".sticker-button"))
      )
        close();
    }
    function escape(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      if (duplicateNotice) {
        setDuplicateNotice(false);
        return;
      }
      if (pendingDelete) {
        setPendingDelete(null);
        return;
      }
      close();
    }
    document.addEventListener("pointerdown", dismiss);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      document.removeEventListener("keydown", escape);
    };
  }, [close, pendingDelete, duplicateNotice]);
  useEffect(() => {
    if (mode !== "emoji") return;
    emojiGrid.current?.scrollTo({ top: 0 });
  }, [mode, emojiCategory]);
  const stickers = library?.stickers ?? [];
  const activeEmojiCategory =
    EMOJI_CATEGORIES.find((category) => category.id === emojiCategory) ??
    EMOJI_CATEGORIES[0];
  if (!activeEmojiCategory) {
    throw new Error("Emoji categories are empty");
  }
  const visibleEmojis = mode === "emoji" ? activeEmojiCategory.emojis : [];
  async function add(file: File) {
    if (!library || uploading) return;
    const active = new AbortController();
    upload.current = active;
    setUploading(true);
    setError("");
    setDuplicateNotice(false);
    setMode("sticker");
    setStatus("Preparing sticker…");
    try {
      const sourceDigest = await stickerDigest(file);
      const blob = await prepareSticker(file, library.max_size);
      const digest = await stickerDigest(blob);
      const { digests, byPath } = await resolveLibraryDigests(library.stickers);
      if (active.signal.aborted) return;
      // Patch digests omitted by older library records so later checks are local.
      if (byPath.size > 0) {
        setLibrary((current) => {
          if (!current) return current;
          return {
            ...current,
            stickers: current.stickers.map((item) => {
              if (item.digest) return item;
              const digest = byPath.get(item.path);
              return digest ? { ...item, digest } : item;
            }),
          };
        });
      }
      // Match both the selected file and the prepared bytes: re-encoding a
      // library WebP changes the prepared digest, but the source still matches.
      if (
        digests.has(sourceDigest) ||
        digests.has(digest) ||
        libraryHasDigest(library.stickers, sourceDigest, digest)
      ) {
        setStatus("");
        setDuplicateNotice(true);
        return;
      }
      setStatus("Uploading sticker…");
      const ticket = await api.allocateSticker(blob.size, active.signal);
      await uploadFile(ticket, blob, active.signal);
      setStatus("Saving sticker…");
      const sticker = await api.completeSticker(ticket.media_id, active.signal);
      if (active.signal.aborted) return;
      setLibrary((current) =>
        current
          ? {
              ...current,
              stickers: [
                { ...sticker, digest: sticker.digest ?? digest },
                ...current.stickers,
              ],
            }
          : current,
      );
      setStatus("");
    } catch (failure) {
      if (active.signal.aborted) return;
      setStatus("");
      if (
        failure instanceof ApiError &&
        failure.status === 409 &&
        failure.message === DUPLICATE_STICKER
      ) {
        setDuplicateNotice(true);
        return;
      }
      setError(errorMessage(failure));
    } finally {
      if (upload.current === active) upload.current = null;
      if (!active.signal.aborted) setUploading(false);
    }
  }
  async function remove(sticker: Sticker) {
    setError("");
    setPendingDelete(null);
    try {
      await api.deleteSticker(sticker.id);
      setLibrary((current) =>
        current
          ? {
              ...current,
              stickers: current.stickers.filter(
                (item) => item.id !== sticker.id,
              ),
            }
          : current,
      );
    } catch (failure) {
      setError(errorMessage(failure));
    }
  }
  const full = !!library && library.stickers.length >= library.limit;
  const addTitle = !library?.enabled
    ? "Stickers need media storage on this server."
    : full
      ? `Sticker limit reached (${library.limit})`
      : `Add a photo or GIF (up to ${formatBytes(library.max_size)})`;
  return (
    <div class="sticker-picker" ref={panel} role="dialog" aria-label="Stickers">
      {mode === "sticker" ? (
        <div class="sticker-grid">
          {stickers.map((sticker) => (
            <div class="sticker-item" key={sticker.id}>
              <button
                type="button"
                class="sticker-choice"
                disabled={disabled}
                aria-label="Insert sticker"
                onClick={() => pick(sticker)}
              >
                <img
                  src={sticker.path}
                  alt=""
                  loading="lazy"
                  decoding="async"
                />
              </button>
              {pendingDelete === sticker.id ? (
                <div
                  class="sticker-delete-confirm"
                  role="dialog"
                  aria-label="Delete this sticker?"
                >
                  <button type="button" onClick={() => setPendingDelete(null)}>
                    Cancel
                  </button>
                  <button
                    type="button"
                    class="sticker-delete-confirm-yes"
                    autoFocus
                    onClick={() => {
                      remove(sticker).catch(() => undefined);
                    }}
                  >
                    Delete
                  </button>
                </div>
              ) : (
                <button
                  type="button"
                  class="sticker-delete"
                  aria-label="Delete sticker"
                  title="Delete sticker"
                  onClick={() => setPendingDelete(sticker.id)}
                >
                  <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="m7 7 10 10M17 7 7 17" />
                  </svg>
                </button>
              )}
            </div>
          ))}
        </div>
      ) : (
        <div class="sticker-emoji-panel">
          <div
            class="sticker-emoji-categories"
            role="tablist"
            aria-label="Emoji categories"
          >
            {EMOJI_CATEGORIES.map((category) => (
              <button
                key={category.id}
                type="button"
                role="tab"
                class="sticker-emoji-category"
                aria-selected={category.id === activeEmojiCategory.id}
                aria-label={category.label}
                title={category.label}
                onClick={() => setEmojiCategory(category.id)}
              >
                <span aria-hidden="true">{category.icon}</span>
              </button>
            ))}
          </div>
          <div
            ref={emojiGrid}
            class="sticker-emoji-grid"
            role="listbox"
            aria-label={activeEmojiCategory.label}
          >
            {visibleEmojis.map((emoji) => (
              <button
                key={emoji}
                type="button"
                class="sticker-emoji-choice"
                aria-label={`Insert ${emoji}`}
                onClick={() => insertEmoji(emoji)}
              >
                {emoji}
              </button>
            ))}
          </div>
        </div>
      )}
      {mode === "sticker" && library && !library.enabled && (
        <p class="sticker-note">Stickers need media storage on this server.</p>
      )}
      {mode === "sticker" &&
        library?.enabled &&
        library.stickers.length === 0 &&
        !status && (
          <p class="sticker-note">No stickers yet. Add a photo or GIF.</p>
        )}
      {mode === "sticker" && (status || error) && (
        <p
          class={error ? "sticker-note sticker-error" : "sticker-note"}
          role="status"
        >
          {error || status}
        </p>
      )}
      {duplicateNotice && (
        <div
          class="sticker-duplicate-notice"
          role="alertdialog"
          aria-labelledby="sticker-duplicate-title"
          aria-describedby="sticker-duplicate-body"
        >
          <p id="sticker-duplicate-title">Can’t add this sticker</p>
          <p id="sticker-duplicate-body">{DUPLICATE_STICKER}</p>
          <button
            type="button"
            autoFocus
            onClick={() => setDuplicateNotice(false)}
          >
            OK
          </button>
        </div>
      )}
      <div class="sticker-dock" role="tablist" aria-label="Picker mode">
        <button
          type="button"
          role="tab"
          class="sticker-dock-tab"
          aria-selected={mode === "emoji"}
          aria-label="Emoji"
          title="Emoji"
          onClick={() => {
            setPendingDelete(null);
            setMode("emoji");
          }}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="12" cy="12" r="8" />
            <path d="M8.75 14.5c.85 1.15 2 1.75 3.25 1.75s2.4-.6 3.25-1.75" />
            <path d="M9.25 9.75h.01M14.75 9.75h.01" />
          </svg>
        </button>
        <button
          type="button"
          role="tab"
          class="sticker-dock-tab"
          aria-selected={mode === "sticker"}
          aria-label="Stickers"
          title="Stickers"
          onClick={() => {
            setPendingDelete(null);
            setMode("sticker");
          }}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M7 4.5h7.2L19.5 9.8V19a1.5 1.5 0 0 1-1.5 1.5H7A1.5 1.5 0 0 1 5.5 19V6A1.5 1.5 0 0 1 7 4.5Z" />
            <path d="M14.2 4.5V9h4.8" />
          </svg>
        </button>
        <label
          class="sticker-dock-tab sticker-dock-add"
          title={addTitle}
          aria-label="Add a sticker"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 5v14M5 12h14" />
          </svg>
          <input
            ref={fileInput}
            type="file"
            accept={stickerAccept}
            disabled={uploading || !library?.enabled || full}
            onChange={(event) => {
              const file = event.currentTarget.files?.[0];
              event.currentTarget.value = "";
              if (file) add(file).catch(() => undefined);
            }}
          />
        </label>
      </div>
    </div>
  );
}
