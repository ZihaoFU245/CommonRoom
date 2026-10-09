import { useState } from "preact/hooks";
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
  onReact,
  onReply,
}: {
  message: Message;
  self: string;
  date: MessageDate;
  grouped: boolean;
  pending: boolean;
  onReact: (value: string) => void;
  onReply: () => void;
}) {
  const [reaction, setReaction] = useState("");
  const [reactionOpen, setReactionOpen] = useState(false);
  const placement = useReactionPlacement(reactionOpen);
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
        {message.reply && (
          <button
            class="reply-quote"
            onClick={() =>
              document
                .getElementById(`message-${message.reply!.id}`)
                ?.scrollIntoView({ block: "center" })
            }
            title="Jump to original message if it is loaded"
          >
            <strong>↳ {message.reply.from}</strong>
            <span dir="auto">{message.reply.text}</span>
          </button>
        )}
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
        <div class="message-reactions">
          {Object.entries(message.reactions || {}).map(([value, users]) => (
            <button
              key={value}
              disabled={pending}
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
      <div class={`message-actions ${reactionOpen ? "open" : ""}`}>
        <button
          type="button"
          disabled={pending}
          onClick={onReply}
          aria-label={`Reply to ${message.from}'s message`}
        >
          Reply
        </button>
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
                aria-label="Custom reaction"
                placeholder="Emoji or text"
                value={reaction}
                onInput={(e) => setReaction(e.currentTarget.value)}
              />
              <button disabled={pending || !reaction.trim()}>Add</button>
            </form>
          </div>
        </details>
      </div>
    </article>
  );
}
