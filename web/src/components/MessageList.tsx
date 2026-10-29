import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { useHtmlGates } from "../hooks/useHtmlGates.ts";
import { useReplyJump } from "../hooks/useReplyJump.ts";
import { HtmlGateContext } from "./HtmlGateContext.ts";
import type { RefObject } from "preact";
import type { VirtualTimelineHandle } from "../hooks/useVirtualTimeline.ts";
import { useVirtualTimeline } from "../hooks/useVirtualTimeline.ts";
import { useTimelineMotion } from "../hooks/useTimelineMotion.ts";
import { ContentMotionContext } from "../hooks/useContentResizeMotion.ts";
import { memo } from "preact/compat";
import type { Command, Message, TimelineEntry } from "../api/protocol.ts";
import {
  messageDate,
  groupedMessage,
  sameMinuteMessage,
} from "../features/conversation/messages.ts";
import { messageActions } from "../features/conversation/permissions.ts";
import { ConsoleOutput } from "./ConsoleOutput.tsx";
import { ConsoleMessage } from "./ConsoleMessage.tsx";
interface Props {
  motionScope: string;
  revision: number;
  historyReady: boolean;
  messageBars?: boolean;
  atBottom: RefObject<boolean>;
  entries: TimelineEntry[];
  username: string;
  avatars?: Record<string, string> | undefined;
  pending: boolean;
  debug: boolean;
  allowHtml: boolean;
  latex: boolean;
  permissions: string[];
  commands: Command[];
  unreadBoundary: number | null;
  onReact: (message: Message, value: string) => void;
  onEdit: (message: Message, value: string) => Promise<boolean>;
  onReply: (message: Message) => void;
  onDelete: (message: Message) => void;
  loadSequence: (sequence: number) => void;
  historyElement: RefObject<HTMLElement>;
  virtualTimeline: RefObject<VirtualTimelineHandle>;
  end: RefObject<HTMLDivElement>;
  onScroll: () => void;
}
export const MessageList = memo(function MessageList({
  motionScope,
  revision,
  historyReady,
  messageBars = false,
  atBottom,
  entries,
  username,
  avatars,
  pending,
  debug,
  allowHtml,
  latex,
  permissions,
  commands,
  unreadBoundary,
  onReact,
  onEdit,
  onReply,
  onDelete,
  loadSequence,
  historyElement,
  virtualTimeline,
  end,
  onScroll,
}: Props) {
  const announcementWatermark = useRef<{
    scope: string;
    through: number;
  } | null>(null);
  const [announcements, setAnnouncements] = useState<Message[]>([]);
  useEffect(() => {
    if (!historyReady) return;
    const messages = entries.flatMap((entry) =>
      entry.kind === "message" ? [entry.message] : [],
    );
    const through = Math.max(0, ...messages.map((message) => message.sequence));
    const previous = announcementWatermark.current;
    // Initial history, backfill and virtual remounts are silent. Only announce
    // arrivals while following this conversation's bottom.
    setAnnouncements(
      previous?.scope === motionScope && atBottom.current
        ? messages
            .filter((message) => message.sequence > previous.through)
            .slice(-5)
        : [],
    );
    announcementWatermark.current = {
      scope: motionScope,
      through:
        previous?.scope === motionScope
          ? Math.max(previous.through, through)
          : through,
    };
  }, [entries, motionScope, historyReady, atBottom]);
  const { gates, gateRevision, expandedMessages, expansion } = useHtmlGates(
    entries,
    motionScope,
    allowHtml,
    atBottom,
  );
  const virtualWindow = useVirtualTimeline(
    entries,
    historyElement,
    virtualTimeline,
    atBottom,
    useMemo(
      () => ({ gateRevision, allowHtml, latex }),
      [gateRevision, allowHtml, latex],
    ),
  );
  useTimelineMotion(
    entries,
    motionScope,
    revision,
    historyReady,
    historyElement,
    atBottom,
    virtualWindow,
  );
  const jumpToReply = useReplyJump(
    entries,
    motionScope,
    historyElement,
    virtualTimeline,
    atBottom,
    loadSequence,
    virtualWindow,
  );
  const renderedEntries = virtualWindow.visible.map((entry, localIndex) => {
    const index = virtualWindow.start + localIndex;
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
    const next = entries[index + 1];
    const connectsPrevious =
      !isUnreadBoundary &&
      previous?.kind === "message" &&
      sameMinuteMessage(entry.message, previous.message);
    const connectsNext =
      next?.kind === "message" &&
      next.message.sequence !== unreadBoundary &&
      sameMinuteMessage(next.message, entry.message);
    const grouped =
      !isUnreadBoundary &&
      previous?.kind === "message" &&
      (messageBars
        ? sameMinuteMessage(entry.message, previous.message)
        : groupedMessage(entry.message, previous.message));
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
          avatar={avatars?.[entry.message.from]}
          expanded={
            expandedMessages.get(entry.message.id) === entry.message.text
          }
          onExpandedChange={expansion}
          self={username}
          date={date}
          grouped={grouped && !entry.message.reply}
          connectsPrevious={connectsPrevious}
          connectsNext={connectsNext}
          pending={pending}
          debug={debug}
          allowHtml={allowHtml}
          latex={latex}
          canReact={actions.react}
          canReply={actions.reply}
          canEdit={actions.edit}
          anchorEditorBottom={index === entries.length - 1}
          canDelete={actions.retract}
          onEdit={onEdit}
          onReact={onReact}
          onDelete={onDelete}
          onReply={onReply}
          onJumpToReply={jumpToReply}
        />
      </div>
    );
  });
  return (
    <ContentMotionContext.Provider value={atBottom}>
      <HtmlGateContext.Provider value={gates}>
        <section
          ref={historyElement}
          onScroll={onScroll}
          class="message-list console-history"
          aria-label="Conversation history"
          tabIndex={0}
        >
          <div class="timeline-rows">
            <div
              aria-hidden="true"
              style={{ height: virtualWindow.top, flexShrink: 0 }}
            />
            {renderedEntries}
            <div
              aria-hidden="true"
              style={{ height: virtualWindow.bottom, flexShrink: 0 }}
            />
            <div ref={end} />
          </div>
        </section>
        <div
          class="visually-hidden"
          role="log"
          aria-label="New messages"
          aria-live="polite"
          aria-relevant="additions"
        >
          {announcements.map((message) => (
            <p key={message.id}>
              {message.from}: {Array.from(message.text).slice(0, 2000).join("")}
            </p>
          ))}
        </div>
      </HtmlGateContext.Provider>
    </ContentMotionContext.Provider>
  );
});
