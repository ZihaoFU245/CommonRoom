import type { RefObject } from "preact";
import { useMemo } from "preact/hooks";
import { memo } from "preact/compat";
import type { Message, TimelineEntry } from "../api/protocol.ts";
import {
  messageDate,
  groupedMessage,
} from "../features/conversation/messages.ts";
import { ConsoleOutput } from "./ConsoleOutput.tsx";
import { ConsoleMessage } from "./ConsoleMessage.tsx";
interface Props {
  entries: TimelineEntry[];
  username: string;
  pending: boolean;
  debug: boolean;
  unreadBoundary: number | null;
  onReact: (message: Message, value: string) => void;
  onReply: (message: Message) => void;
  onDelete: (message: Message) => void;
  historyElement: RefObject<HTMLElement>;
  end: RefObject<HTMLDivElement>;
  onScroll: () => void;
}
export const MessageList = memo(function MessageList({
  entries,
  username,
  pending,
  debug,
  unreadBoundary,
  onReact,
  onReply,
  onDelete,
  historyElement,
  end,
  onScroll,
}: Props) {
  const renderedEntries = useMemo(() => {
    let lastDay: string | null = null;
    return entries.map((entry, index) => {
      if (entry.kind !== "message")
        return <ConsoleOutput key={entry.key} entry={entry} />;
      const date = messageDate(entry.message.time);
      const newDay = lastDay !== date.key;
      lastDay = date.key;
      const previous = entries[index - 1];
      const isUnreadBoundary = entry.message.sequence === unreadBoundary;
      const grouped =
        !isUnreadBoundary &&
        previous?.kind === "message" &&
        groupedMessage(entry.message, previous.message);
      return (
        <div key={entry.key}>
          {newDay && (
            <div class="message-day">
              <time dateTime={date.iso}>{date.label}</time>
            </div>
          )}
          {isUnreadBoundary && <div class="unread-divider">New messages</div>}
          <ConsoleMessage
            message={entry.message}
            self={username}
            date={date}
            grouped={grouped && !entry.message.reply}
            pending={pending}
            debug={debug}
            onReact={(value) => onReact(entry.message, value)}
            onDelete={() => onDelete(entry.message)}
            onReply={() => {
              onReply(entry.message);
            }}
          />
        </div>
      );
    });
  }, [
    entries,
    username,
    pending,
    debug,
    unreadBoundary,
    onReact,
    onReply,
    onDelete,
  ]);
  return (
    <section
      ref={historyElement}
      onScroll={onScroll}
      class="message-list console-history"
      aria-label="Conversation history"
      aria-live="polite"
    >
      {renderedEntries}
      <div ref={end} />
    </section>
  );
});
