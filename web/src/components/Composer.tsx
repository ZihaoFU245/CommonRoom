import type { Attachment } from "../hooks/useMediaUploads.ts";
import type { JSX, RefObject } from "preact";
import type { Hint, ReplyTarget } from "../api/protocol.ts";
import { useEffect, useRef } from "preact/hooks";
interface Props {
  mediaUpload?: {
    enabled: boolean;
    maxSize: number;
    attachment: Attachment | null;
    busy: boolean;
    status: string;
    error: string;
    start: (file: File) => Promise<void>;
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
  useEffect(() => {
    hintList.current
      ?.querySelector('[aria-selected="true"]')
      ?.scrollIntoView({ block: "nearest" });
  }, [activeHint, hints.length]);
  return (
    <div class="composer-area">
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
        <div class="composer-attachment">
          {mediaUpload.attachment.preview && (
            <img
              src={mediaUpload.attachment.preview}
              alt={mediaUpload.attachment.media.filename}
            />
          )}
          <span>
            {mediaUpload.attachment.media.filename}
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
          {mediaUpload?.enabled && !consoleView && canWrite && (
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
                disabled={
                  pending ||
                  mediaUpload.busy ||
                  !!mediaUpload.attachment ||
                  draft.trim().startsWith("/")
                }
                onChange={(event) => {
                  const file = event.currentTarget.files?.[0];
                  event.currentTarget.value = "";
                  if (file) mediaUpload.start(file).catch(() => undefined);
                }}
              />
            </label>
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
            onKeyDown={keydown}
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
              (!draft.trim() && !mediaUpload?.attachment) ||
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
