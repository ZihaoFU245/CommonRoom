import type { RefObject } from "preact";
import type { VirtualTimelineHandle } from "../hooks/useVirtualTimeline.ts";
import { useVirtualTimeline } from "../hooks/useVirtualTimeline.ts";
import { memo } from "preact/compat";
import type { Command, Message, TimelineEntry } from "../api/protocol.ts";
import {
  messageDate,
  groupedMessage,
} from "../features/conversation/messages.ts";
import { messageActions } from "../features/conversation/permissions.ts";
import { ConsoleOutput } from "./ConsoleOutput.tsx";
import { ConsoleMessage } from "./ConsoleMessage.tsx";
interface Props {
  entries: TimelineEntry[];
  username: string;
  pending: boolean;
  debug: boolean;
  permissions: string[];
  commands: Command[];
  unreadBoundary: number | null;
  onReact: (message: Message, value: string) => void;
  onEdit: (message: Message, value: string) => Promise<boolean>;
  onReply: (message: Message) => void;
  onDelete: (message: Message) => void;
  historyElement: RefObject<HTMLElement>;
  virtualTimeline: RefObject<VirtualTimelineHandle>;
  end: RefObject<HTMLDivElement>;
  onScroll: () => void;
}
export const MessageList = memo(function MessageList({
  entries,
  username,
  pending,
  debug,
  permissions,
  commands,
  unreadBoundary,
  onReact,
  onEdit,
  onReply,
  onDelete,
  historyElement,
  virtualTimeline,
  end,
  onScroll,
}: Props) {
  const window = useVirtualTimeline(entries, historyElement, virtualTimeline);
  const renderedEntries = window.visible.map((entry, localIndex) => {
    const index = window.start + localIndex;
    if (entry.kind !== "message")
      return (
        <div key={entry.key} data-virtual-key={entry.key}>
          <ConsoleOutput entry={entry} />
        </div>
      );
    const actions = messageActions(
      entry.message,
      username,
      permissions,
      commands,
    );
    const date = messageDate(entry.message.time);
    const previous = entries[index - 1];
    const newDay =
      previous?.kind !== "message" ||
      messageDate(previous.message.time).key !== date.key;
    const isUnreadBoundary = entry.message.sequence === unreadBoundary;
    const grouped =
      !isUnreadBoundary &&
      previous?.kind === "message" &&
      groupedMessage(entry.message, previous.message);
    return (
      <div key={entry.key} data-virtual-key={entry.key}>
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
          canReact={actions.react}
          canReply={actions.reply}
          canEdit={actions.edit}
          canDelete={actions.retract}
          onEdit={(value) => onEdit(entry.message, value)}
          onReact={(value) => onReact(entry.message, value)}
          onDelete={() => onDelete(entry.message)}
          onReply={() => {
            onReply(entry.message);
          }}
        />
      </div>
    );
  });
  return (
    <section
      ref={historyElement}
      onScroll={onScroll}
      class="message-list console-history"
      aria-label="Conversation history"
      aria-live="polite"
    >
      <div aria-hidden="true" style={{ height: window.top, flexShrink: 0 }} />
      {renderedEntries}
      <div
        aria-hidden="true"
        style={{ height: window.bottom, flexShrink: 0 }}
      />
      <div ref={end} />
    </section>
  );
});
