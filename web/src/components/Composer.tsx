import type { JSX, RefObject } from "preact";
import type { Hint, ReplyTarget } from "../api/protocol.ts";
interface Props {
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
}
export function Composer({
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
}: Props) {
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
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => acceptHint(hint)}
                disabled={!hint.value}
              >
                <code>{hint.label}</code>
                <span>{hint.description}</span>
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
      <form class="composer" onSubmit={send}>
        {consoleView && (
          <span class="prompt" aria-hidden="true">
            ›
          </span>
        )}
        <textarea
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
          disabled={pending}
          onInput={(event) => edit(event.currentTarget.value)}
          onKeyDown={keydown}
        />
        <button
          class="send-button"
          disabled={
            pending ||
            !draft.trim() ||
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
      </form>
    </div>
  );
}
