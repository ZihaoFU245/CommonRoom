import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { uploadFile } from "../api/media.ts";
import { errorMessage } from "../api/protocol.ts";
import type { Sticker, StickerLibrary } from "../api/stickers.ts";
import {
  formatBytes,
  prepareSticker,
  stickerAccept,
} from "../features/conversation/stickers.ts";

interface Props {
  disabled: boolean;
  pick: (sticker: Sticker) => void;
  close: () => void;
}

const COLUMNS = 4;
const ROWS = 3;
/** One cell is reserved for the add control in the top-right corner. */
const VISIBLE = COLUMNS * ROWS - 1;
const ROW_STEP = COLUMNS;

export function StickerPicker({ disabled, pick, close }: Props) {
  const [library, setLibrary] = useState<StickerLibrary | null>(null);
  const [status, setStatus] = useState("Loading stickers…");
  const [error, setError] = useState("");
  const [uploading, setUploading] = useState(false);
  const [offset, setOffset] = useState(0);
  const [roll, setRoll] = useState<"down" | "up" | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const grid = useRef<HTMLDivElement>(null);
  const upload = useRef<AbortController | null>(null);
  const rollTimer = useRef<ReturnType<typeof setTimeout>>();
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
      clearTimeout(rollTimer.current);
    };
  }, []);
  useEffect(() => {
    function dismiss(event: PointerEvent) {
      const target = event.target;
      if (
        target instanceof Node &&
        !panel.current?.contains(target) &&
        !(target instanceof Element && target.closest(".sticker-button"))
      )
        close();
    }
    function escape(event: KeyboardEvent) {
      if (event.key === "Escape") close();
    }
    document.addEventListener("pointerdown", dismiss);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      document.removeEventListener("keydown", escape);
    };
  }, [close]);
  const stickers = library?.stickers ?? [];
  const maxOffset = Math.max(0, stickers.length - VISIBLE);
  useEffect(() => {
    setOffset((current) => Math.min(current, maxOffset));
  }, [maxOffset]);
  useEffect(() => {
    const node = grid.current;
    if (!node) return;
    function onWheel(event: WheelEvent) {
      if (maxOffset === 0) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.deltaY === 0) return;
      const direction = event.deltaY > 0 ? "down" : "up";
      setOffset((current) => {
        const next =
          direction === "down"
            ? Math.min(current + ROW_STEP, maxOffset)
            : Math.max(current - ROW_STEP, 0);
        if (next === current) return current;
        setRoll(direction);
        clearTimeout(rollTimer.current);
        rollTimer.current = setTimeout(() => setRoll(null), 220);
        return next;
      });
    }
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  }, [maxOffset]);
  async function add(file: File) {
    if (!library || uploading) return;
    const active = new AbortController();
    upload.current = active;
    setUploading(true);
    setError("");
    setStatus("Preparing sticker…");
    try {
      const blob = await prepareSticker(file, library.max_size);
      setStatus("Uploading sticker…");
      const ticket = await api.allocateSticker(blob.size, active.signal);
      await uploadFile(ticket, blob, active.signal);
      setStatus("Saving sticker…");
      const sticker = await api.completeSticker(ticket.media_id, active.signal);
      if (active.signal.aborted) return;
      setLibrary((current) =>
        current
          ? { ...current, stickers: [sticker, ...current.stickers] }
          : current,
      );
      setOffset(0);
      setStatus("");
    } catch (failure) {
      if (active.signal.aborted) return;
      setStatus("");
      setError(errorMessage(failure));
    } finally {
      if (upload.current === active) upload.current = null;
      if (!active.signal.aborted) setUploading(false);
    }
  }
  async function remove(sticker: Sticker) {
    setError("");
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
  const visible = stickers.slice(offset, offset + VISIBLE);
  return (
    <div class="sticker-picker" ref={panel} role="dialog" aria-label="Stickers">
      <div
        class={roll ? `sticker-grid sticker-roll-${roll}` : "sticker-grid"}
        ref={grid}
      >
        {library?.enabled && (
          <label
            class="sticker-add"
            title={
              full
                ? `Sticker limit reached (${library.limit})`
                : `Add a photo or GIF (up to ${formatBytes(library.max_size)})`
            }
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M12 5v14M5 12h14" />
            </svg>
            <input
              type="file"
              accept={stickerAccept}
              aria-label="Add a sticker"
              disabled={uploading || full}
              onChange={(event) => {
                const file = event.currentTarget.files?.[0];
                event.currentTarget.value = "";
                if (file) add(file).catch(() => undefined);
              }}
            />
          </label>
        )}
        {visible.map((sticker) => (
          <div class="sticker-item" key={`${offset}:${sticker.id}`}>
            <button
              type="button"
              class="sticker-choice"
              disabled={disabled}
              aria-label="Insert sticker"
              onClick={() => pick(sticker)}
            >
              <img src={sticker.path} alt="" loading="lazy" decoding="async" />
            </button>
            <button
              type="button"
              class="sticker-delete"
              aria-label="Delete sticker"
              title="Delete sticker"
              onClick={() => {
                remove(sticker).catch(() => undefined);
              }}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="m7 7 10 10M17 7 7 17" />
              </svg>
            </button>
          </div>
        ))}
      </div>
      {library && !library.enabled && (
        <p class="sticker-note">Stickers need media storage on this server.</p>
      )}
      {library?.enabled && library.stickers.length === 0 && !status && (
        <p class="sticker-note">No stickers yet. Add a photo or GIF.</p>
      )}
      {(status || error) && (
        <p
          class={error ? "sticker-note sticker-error" : "sticker-note"}
          role="status"
        >
          {error || status}
        </p>
      )}
    </div>
  );
}
