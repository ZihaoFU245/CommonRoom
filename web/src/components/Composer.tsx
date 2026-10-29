import type { Attachment } from "../hooks/useMediaUploads.ts";
import type { JSX, RefObject } from "preact";
import type { Hint, ReplyTarget } from "../api/protocol.ts";
import type { Sticker } from "../api/stickers.ts";
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { StickerPicker } from "./StickerPicker.tsx";
import {
  canAttachFile,
  clipboardAttachment,
} from "../features/conversation/attachments.ts";
import { hasMessageContent } from "../features/conversation/messageContent.ts";
import { useCompositionGuard } from "../hooks/useCompositionGuard.ts";
interface Props {
  mediaUpload?: {
    enabled: boolean;
    maxSize: number;
    attachment: Attachment | null;
    busy: boolean;
    status: string;
    error: string;
    start: (file: File) => Promise<void>;
    attachSticker: (sticker: Sticker) => Promise<void>;
    cancel: () => void;
    retry: (() => Promise<void>) | undefined;
  };
  minimumComposerHeight: number;
  actualComposerHeight: number;
  hints: Hint[];
  activeHint: number;
  replyTarget: ReplyTarget | null;
  input: RefObject<HTMLTextAreaElement>;
  consoleView: boolean;
  draft: string;
  pending: boolean;
  canWrite: boolean;
  startResize: JSX.PointerEventHandler<HTMLDivElement>;
  moveResize: JSX.PointerEventHandler<HTMLDivElement>;
  endResize: () => void;
  resizeComposer: (height: number) => void;
  acceptHint: (hint: Hint) => void;
  cancelReply: () => void;
  send: JSX.SubmitEventHandler<HTMLFormElement>;
  edit: (value: string) => void;
  keydown: JSX.KeyboardEventHandler<HTMLTextAreaElement>;
  selectCaret: () => void;
}
export function Composer({
  mediaUpload,
  minimumComposerHeight,
  actualComposerHeight,
  hints,
  activeHint,
  replyTarget,
  input,
  consoleView,
  draft,
  pending,
  canWrite,
  startResize,
  moveResize,
  endResize,
  resizeComposer,
  acceptHint,
  cancelReply,
  send,
  edit,
  keydown,
  selectCaret,
}: Props) {
  const hintList = useRef<HTMLUListElement>(null);
  const composition = useCompositionGuard(keydown);
  const [stickers, setStickers] = useState(false);
  const closeStickers = useCallback(() => setStickers(false), []);
  useEffect(() => {
    hintList.current
      ?.querySelector('[aria-selected="true"]')
      ?.scrollIntoView({ block: "nearest" });
  }, [activeHint, hints.length]);
  const attachVisible = !!mediaUpload?.enabled && !consoleView && canWrite;
  const attachAllowed =
    !!mediaUpload &&
    canAttachFile({
      enabled: mediaUpload.enabled,
      consoleView,
      canWrite,
      pending,
      uploading: mediaUpload.busy,
      attached: !!mediaUpload.attachment,
      draft,
    });
  // A pasted image becomes the attachment the file picker would produce.
  function paste(event: JSX.TargetedClipboardEvent<HTMLTextAreaElement>) {
    if (!mediaUpload || !attachAllowed) return;
    const file = clipboardAttachment(event.clipboardData);
    if (!file) return;
    event.preventDefault();
    mediaUpload.start(file).catch(() => undefined);
  }
  return (
    <div class="composer-area">
      {mediaUpload && attachVisible && stickers && (
        <StickerPicker
          disabled={!attachAllowed}
          close={closeStickers}
          pick={(sticker) => {
            setStickers(false);
            mediaUpload.attachSticker(sticker).catch(() => undefined);
            input.current?.focus();
          }}
        />
      )}
      <div
        class="composer-resize"
        role="separator"
        tabIndex={0}
        aria-label="Resize message input"
        aria-orientation="horizontal"
        aria-valuemin={minimumComposerHeight}
        aria-valuemax={Math.floor(Math.min(240, window.innerHeight * 0.4))}
        aria-valuenow={actualComposerHeight}
        onPointerDown={startResize}
        onPointerMove={moveResize}
        onPointerUp={endResize}
        onPointerCancel={endResize}
        onLostPointerCapture={endResize}
        onKeyDown={(event) => {
          if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
            event.preventDefault();
            resizeComposer(
              event.key === "Home"
                ? minimumComposerHeight
                : event.key === "End"
                  ? 240
                  : actualComposerHeight + (event.key === "ArrowUp" ? 16 : -16),
            );
          }
        }}
      >
        <span />
      </div>
      {hints.length > 0 && (
        <ul
          class="command-hints"
          ref={hintList}
          id="command-hints"
          role="listbox"
          aria-label="Command suggestions"
        >
          {hints.map((hint, index) => (
            <li
              id={`hint-${index}`}
              key={hint.label}
              role="option"
              aria-selected={index === activeHint}
            >
              <button
                type="button"
                tabIndex={-1}
                class={index === activeHint ? "active" : ""}
                title={hint.description || undefined}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => acceptHint(hint)}
                disabled={!hint.value}
              >
                <code>{hint.label}</code>
              </button>
            </li>
          ))}
        </ul>
      )}
      {replyTarget && (
        <div class="reply-composer">
          <div>
            <strong>Reply to {replyTarget.from}</strong>
            <p dir="auto">
              {Array.from(replyTarget.text).slice(0, 160).join("")}
            </p>
          </div>
          <button
            type="button"
            aria-label="Cancel reply"
            onClick={() => cancelReply()}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="m6 6 12 12M18 6 6 18" />
            </svg>
          </button>
        </div>
      )}
      {mediaUpload && (mediaUpload.status || mediaUpload.error) && (
        <div class="media-upload-status" role="status">
          <span>{mediaUpload.error || mediaUpload.status}</span>
          {mediaUpload.busy && (
            <button type="button" onClick={mediaUpload.cancel}>
              Cancel upload
            </button>
          )}
          {mediaUpload.retry && (
            <button
              type="button"
              onClick={() => {
                mediaUpload.retry?.().catch(() => undefined);
              }}
            >
              Retry upload
            </button>
          )}
        </div>
      )}
      {mediaUpload?.attachment && (
        <div
          class={
            mediaUpload.attachment.sticker
              ? "composer-attachment composer-sticker"
              : "composer-attachment"
          }
        >
          {mediaUpload.attachment.preview && (
            <img
              src={mediaUpload.attachment.preview}
              alt={mediaUpload.attachment.media.filename}
            />
          )}
          <span>
            {mediaUpload.attachment.sticker
              ? "Sticker"
              : mediaUpload.attachment.media.filename}
            <small>
              {mediaUpload.attachment.media.size.toLocaleString()} bytes
            </small>
          </span>
          <button
            type="button"
            disabled={pending}
            onClick={mediaUpload.cancel}
            aria-label="Remove attachment"
          >
            Remove
          </button>
        </div>
      )}
      <form class="composer" onSubmit={send}>
        <div class="composer-input">
          {mediaUpload && attachVisible && (
            <label
              class="attach-button"
              title={`Attach a file (up to ${mediaUpload.maxSize.toLocaleString()} bytes)`}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="M12 5v14M5 12h14" />
              </svg>
              <input
                type="file"
                aria-label="Attach a file"
                disabled={!attachAllowed}
                onChange={(event) => {
                  const file = event.currentTarget.files?.[0];
                  event.currentTarget.value = "";
                  if (file) mediaUpload.start(file).catch(() => undefined);
                }}
              />
            </label>
          )}
          {mediaUpload && attachVisible && (
            <button
              type="button"
              class="attach-button sticker-button"
              aria-label="Stickers"
              aria-expanded={stickers}
              aria-haspopup="dialog"
              title="Stickers"
              onClick={() => setStickers((open) => !open)}
            >
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <circle cx="12" cy="12" r="8.5" />
                <path d="M8.5 14.5c.9 1.2 2.1 1.8 3.5 1.8s2.6-.6 3.5-1.8" />
                <path d="M9.25 9.75h.01M14.75 9.75h.01" />
              </svg>
            </button>
          )}
          {consoleView && (
            <span class="prompt" aria-hidden="true">
              ›
            </span>
          )}
          <textarea
            name="message"
            ref={input}
            dir="auto"
            rows={1}
            style={{ height: `${actualComposerHeight}px` }}
            aria-label="Message or command"
            aria-autocomplete="list"
            aria-controls={hints.length ? "command-hints" : undefined}
            aria-activedescendant={
              hints.length ? `hint-${activeHint}` : undefined
            }
            placeholder={
              consoleView
                ? "Command"
                : canWrite
                  ? "Message or command"
                  : "Read-only conversation · Enter a command"
            }
            value={draft}
            onInput={(event) => edit(event.currentTarget.value)}
            onPaste={paste}
            {...composition}
            onSelect={selectCaret}
            onClick={selectCaret}
            onKeyUp={selectCaret}
          />
          <button
            class="send-button"
            disabled={
              pending ||
              mediaUpload?.busy ||
              (!!mediaUpload?.attachment && !mediaUpload.enabled) ||
              (!hasMessageContent(draft) && !mediaUpload?.attachment) ||
              (!canWrite && !draft.trim().startsWith("/"))
            }
            aria-label="Send message"
          >
            {pending ? (
              "…"
            ) : (
              <svg viewBox="0 0 24 24" aria-hidden="true">
                <path d="M12 20V4m-6 6 6-6 6 6" />
              </svg>
            )}
          </button>
        </div>
      </form>
    </div>
  );
}
