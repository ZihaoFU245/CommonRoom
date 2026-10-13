import { Markdown } from "./Markdown.tsx";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import type { Message } from "../api/protocol.ts";
import { mentionedText } from "../features/conversation/interactions.ts";
import type { MessageDate } from "../features/conversation/messages.ts";
import { useReactionPlacement } from "../hooks/useReactionPlacement.ts";

export function ConsoleMessage({
  message,
  self,
  date,
  grouped,
  pending,
  debug,
  canReact,
  canReply,
  canDelete,
  canEdit,
  onEdit,
  onReact,
  onReply,
  onDelete,
}: {
  message: Message;
  self: string;
  date: MessageDate;
  grouped: boolean;
  pending: boolean;
  debug: boolean;
  canReact: boolean;
  canReply: boolean;
  canDelete: boolean;
  canEdit: boolean;
  onEdit: (value: string) => Promise<boolean>;
  onReact: (value: string) => void;
  onReply: () => void;
  onDelete: () => void;
}) {
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const editor = useRef<HTMLTextAreaElement>(null);
  const messageBody = useRef<HTMLFormElement>(null);
  const bodyAnimation = useRef<Animation | null>(null);
  const previousHeight = useRef<number | null>(null);
  function changeEditing(next: boolean) {
    previousHeight.current =
      messageBody.current?.getBoundingClientRect().height ?? null;
    bodyAnimation.current?.cancel();
    setEditing(next);
  }
  useLayoutEffect(() => {
    const body = messageBody.current;
    const from = previousHeight.current;
    previousHeight.current = null;
    if (
      !body ||
      from === null ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    )
      return;
    const to = body.getBoundingClientRect().height;
    // Animate the persistent body only. Removed inputs must never be reinserted.
    bodyAnimation.current = body.animate(
      [
        { height: `${from}px`, overflow: "hidden" },
        { height: `${to}px`, overflow: "hidden" },
      ],
      { duration: 180, easing: "ease-out" },
    );
  }, [editing, canEdit]);
  useEffect(() => () => bodyAnimation.current?.cancel(), []);
  useEffect(() => {
    if (editing && canEdit) editor.current?.focus();
  }, [editing, canEdit]);
  const [editText, setEditText] = useState("");
  const editOriginal = useRef("");
  const [editError, setEditError] = useState("");
  const [reaction, setReaction] = useState("");
  const [reactionOpen, setReactionOpen] = useState(false);
  const placement = useReactionPlacement(reactionOpen);
  useEffect(() => {
    if (!reactionOpen) return;
    const dismissOutside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !placement.anchor.current?.contains(event.target)
      ) {
        setReactionOpen(false);
      }
    };
    const dismissEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setReactionOpen(false);
        placement.anchor.current?.querySelector("summary")?.focus();
      }
    };
    document.addEventListener("pointerdown", dismissOutside, true);
    document.addEventListener("keydown", dismissEscape);
    return () => {
      document.removeEventListener("pointerdown", dismissOutside, true);
      document.removeEventListener("keydown", dismissEscape);
    };
  }, [reactionOpen, placement.anchor]);
  const reply = message.reply;
  const text = mentionedText(message.text, message.mentions);
  return (
    <article
      id={`message-${message.id}`}
      data-sequence={message.sequence}
      class={`chat-message ${message.from === self ? "own" : ""} ${grouped ? "grouped" : ""}`}
      title={date.full}
    >
      {grouped && (
        <time
          class="continuation-time"
          dateTime={date.iso}
          aria-label={date.full}
        >
          {date.time}
        </time>
      )}
      <div class="message-content">
        {!grouped && (
          <header class="message-meta">
            <strong class="message-author">{message.from}</strong>
            <time dateTime={date.iso} title={date.full} aria-label={date.full}>
              {date.time}
            </time>
          </header>
        )}
        {reply && (
          <button
            class="reply-quote"
            onClick={() =>
              document
                .getElementById(`message-${reply.id}`)
                ?.scrollIntoView({ block: "center" })
            }
            title="Jump to original message if it is loaded"
          >
            <strong>↳ {reply.from}</strong>
            <span dir="auto">{reply.text}</span>
          </button>
        )}
        <form
          ref={messageBody}
          class={editing && canEdit ? "message-editor" : "message-body"}
          onSubmit={(event) => {
            event.preventDefault();
            if (
              !editing ||
              !canEdit ||
              pending ||
              !editText.trim() ||
              Array.from(editText.trim()).length > 4000
            )
              return;
            setEditError("");
            if (editText.trim() === editOriginal.current) {
              changeEditing(false);
              return;
            }
            onEdit(editText).then(
              (saved) => {
                if (saved) changeEditing(false);
                else setEditError("Could not save your edit. Try again.");
              },
              () => setEditError("Could not save your edit. Try again."),
            );
          }}
        >
          {editing && canEdit ? (
            <>
              <textarea
                name="message-edit"
                aria-label="Edit your message"
                ref={editor}
                value={editText}
                disabled={pending}
                onInput={(event) => setEditText(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape" && !pending) {
                    event.preventDefault();
                    changeEditing(false);
                  }
                }}
              />
              {editError && <p role="alert">{editError}</p>}
              <div>
                <button
                  type="submit"
                  class="primary"
                  disabled={
                    pending ||
                    !editText.trim() ||
                    Array.from(editText.trim()).length > 4000
                  }
                >
                  Save
                </button>
                <button
                  type="button"
                  disabled={pending}
                  onClick={() => changeEditing(false)}
                >
                  Cancel
                </button>
              </div>
            </>
          ) : message.markdown ? (
            <Markdown text={message.text} mentions={message.mentions} />
          ) : (
            <p dir="auto">
              {text.map((part, index) =>
                part.mention ? (
                  <mark key={index} class="mention">
                    {part.text}
                  </mark>
                ) : (
                  part.text
                ),
              )}
            </p>
          )}
        </form>
        {message.edited && <span class="message-edited">edited</span>}
        {debug && detailsOpen && (
          <dl id={`details-${message.id}`} class="message-debug">
            <dt>Message ID</dt>
            <dd>
              <code>{message.id}</code>
            </dd>
            <dt>Sender</dt>
            <dd>{message.from}</dd>
            <dt>Recipient</dt>
            <dd>{message.to ?? "Room"}</dd>
            <dt>Timestamp</dt>
            <dd>
              {date.full} ({message.time})
            </dd>
            <dt>Sequence</dt>
            <dd>{message.sequence}</dd>
          </dl>
        )}
        <div class="message-reactions">
          {Object.entries(message.reactions || {}).map(([value, users]) => (
            <button
              key={value}
              disabled={pending || !canReact}
              aria-pressed={users.includes(self)}
              title={users.join(", ")}
              aria-label={`React ${value}: ${users.length}`}
              onClick={() => onReact(value)}
            >
              {value}
              <span>{users.length}</span>
            </button>
          ))}
        </div>
      </div>
      <div
        class={`message-actions ${editing || reactionOpen || (debug && detailsOpen) ? "open" : ""}`}
      >
        {canReply && (
          <button
            type="button"
            disabled={pending}
            onClick={onReply}
            aria-label={`Reply to ${message.from}'s message`}
          >
            Reply
          </button>
        )}
        {canEdit && (
          <button
            type="button"
            disabled={pending || editing}
            aria-label="Edit your message"
            onClick={() => {
              editOriginal.current = message.text;
              setEditText(message.text);
              setEditError("");
              changeEditing(true);
            }}
          >
            Edit
          </button>
        )}
        {canDelete && (
          <button
            type="button"
            disabled={pending}
            onClick={onDelete}
            aria-label={
              message.from === self ? "Delete your message" : "Delete message"
            }
          >
            Delete
          </button>
        )}
        {canReact && (
          <details
            ref={placement.anchor}
            open={reactionOpen}
            onToggle={(event) => setReactionOpen(event.currentTarget.open)}
          >
            <summary
              aria-label={`Add reaction to ${message.from}'s message`}
              onClick={(event) => {
                event.preventDefault();
                setReactionOpen(!reactionOpen);
              }}
            >
              +
            </summary>
            <div
              ref={placement.picker}
              class={`reaction-picker ${placement.above ? "above" : ""}`}
            >
              <div class="reaction-choices">
                {["👍", "❤️", "😂", "🎉", "👀"].map((value) => (
                  <button
                    key={value}
                    type="button"
                    disabled={pending}
                    aria-label={`Add reaction ${value}`}
                    onClick={() => {
                      onReact(value);
                      setReactionOpen(false);
                    }}
                  >
                    {value}
                  </button>
                ))}
              </div>
              <form
                onSubmit={(event) => {
                  event.preventDefault();
                  onReact(reaction);
                  setReaction("");
                  setReactionOpen(false);
                }}
              >
                <input
                  name="reaction"
                  aria-label="Custom reaction"
                  placeholder="Emoji or text"
                  value={reaction}
                  onInput={(e) => setReaction(e.currentTarget.value)}
                />
                <button disabled={pending || !reaction.trim()}>Add</button>
              </form>
            </div>
          </details>
        )}
        {debug && (
          <button
            type="button"
            aria-label="Show message details"
            aria-expanded={detailsOpen}
            aria-controls={`details-${message.id}`}
            onClick={() => setDetailsOpen(!detailsOpen)}
          >
            ⋯
          </button>
        )}
      </div>
    </article>
  );
}
