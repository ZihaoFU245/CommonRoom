import { Markdown } from "./Markdown.tsx";
import { useEffect, useRef, useState } from "preact/hooks";
import type { Message } from "../api/protocol.ts";
import { mentionedText } from "../features/conversation/interactions.ts";
import type { MessageDate } from "../features/conversation/messages.ts";
import { useMessageEditorLayout } from "../hooks/useMessageEditorLayout.ts";
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
  anchorEditorBottom = false,
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
  anchorEditorBottom?: boolean;
  onEdit: (value: string) => Promise<boolean>;
  onReact: (value: string) => void;
  onReply: () => void;
  onDelete: () => void;
}) {
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState("");
  const {
    input: editor,
    body: messageBody,
    prepareTransition,
  } = useMessageEditorLayout({
    active: editing && canEdit,
    text: editText,
    anchorBottom: anchorEditorBottom,
  });
  function changeEditing(next: boolean) {
    prepareTransition();
    setEditing(next);
  }
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
  const editValue = editText.trim();
  const validEdit =
    editing && !!editValue && Array.from(editValue).length <= 4000;
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
            if (!editing || !canEdit || pending || !validEdit) return;
            setEditError("");
            if (editValue === editOriginal.current) {
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
                dir="auto"
                rows={1}
                aria-label={
                  message.from === self ? "Edit your message" : "Edit message"
                }
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
                  disabled={pending || !validEdit}
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
            aria-label={
              message.from === self ? "Edit your message" : "Edit message"
            }
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
