import { useMemo, useState } from "preact/hooks";
import type { HtmlGateChoice, HtmlGateMode } from "./HtmlGateContext.ts";
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
} from "../features/conversation/messages.ts";
import { messageActions } from "../features/conversation/permissions.ts";
import { ConsoleOutput } from "./ConsoleOutput.tsx";
import { ConsoleMessage } from "./ConsoleMessage.tsx";
interface Props {
  motionScope: string;
  revision: number;
  historyReady: boolean;
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
  historyElement: RefObject<HTMLElement>;
  virtualTimeline: RefObject<VirtualTimelineHandle>;
  end: RefObject<HTMLDivElement>;
  onScroll: () => void;
}
export const MessageList = memo(function MessageList({
  motionScope,
  revision,
  historyReady,
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
  historyElement,
  virtualTimeline,
  end,
  onScroll,
}: Props) {
  // Bound choices to the current history window and conversation, surviving row unmounts.
  const choices = useMemo(
    () => new Map<string, HtmlGateChoice>(),
    [motionScope, allowHtml],
  );
  const [gateRevision, setGateRevision] = useState(0);
  const liveMessages = useMemo(() => {
    const messages = new Map<string, string>();
    if (allowHtml) {
      for (const entry of entries) {
        if (entry.kind === "message")
          messages.set(entry.message.id, entry.message.text);
      }
    }
    return messages;
  }, [entries, allowHtml]);
  const gates = useMemo(() => {
    for (const [key, choice] of choices) {
      if (liveMessages.get(choice.messageKey) !== choice.text)
        choices.delete(key);
    }
    return {
      choices,
      change: (
        key: string,
        messageKey: string,
        text: string,
        mode: HtmlGateMode,
      ) => {
        const previous = choices.get(key);
        if (previous?.text === text && previous.mode === mode) return;
        if (mode) choices.set(key, { messageKey, text, mode });
        else choices.delete(key);
        setGateRevision((value) => value + 1);
      },
    };
  }, [choices, liveMessages, gateRevision]);
  const expandedMessages = useMemo(
    () => new Map<string, string>(),
    [motionScope],
  );
  const expansion = useMemo(() => {
    const live = new Map(
      entries.flatMap((entry) =>
        entry.kind === "message"
          ? [[entry.message.id, entry.message.text]]
          : [],
      ),
    );
    for (const [key, text] of expandedMessages) {
      if (live.get(key) !== text) expandedMessages.delete(key);
    }
    return (message: Message, expanded: boolean) => {
      atBottom.current = false;
      if (expanded) expandedMessages.set(message.id, message.text);
      else expandedMessages.delete(message.id);
      setGateRevision((value) => value + 1);
    };
  }, [entries, expandedMessages, atBottom]);
  const window = useVirtualTimeline(
    entries,
    historyElement,
    virtualTimeline,
    atBottom,
    gateRevision * 4 + (allowHtml ? 1 : 0) + (latex ? 2 : 0),
  );
  useTimelineMotion(
    entries,
    motionScope,
    revision,
    historyReady,
    historyElement,
    atBottom,
    window,
  );
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
          avatar={avatars?.[entry.message.from]}
          expanded={
            expandedMessages.get(entry.message.id) === entry.message.text
          }
          onExpandedChange={expansion}
          self={username}
          date={date}
          grouped={grouped && !entry.message.reply}
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
          aria-live="polite"
        >
          <div class="timeline-rows">
            <div
              aria-hidden="true"
              style={{ height: window.top, flexShrink: 0 }}
            />
            {renderedEntries}
            <div
              aria-hidden="true"
              style={{ height: window.bottom, flexShrink: 0 }}
            />
            <div ref={end} />
          </div>
        </section>
      </HtmlGateContext.Provider>
    </ContentMotionContext.Provider>
  );
});
