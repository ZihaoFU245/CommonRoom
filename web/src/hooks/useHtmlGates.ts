import type { RefObject } from "preact";
import { useMemo, useState } from "preact/hooks";
import type { Message, TimelineEntry } from "../api/protocol.ts";
import type {
  HtmlGateChoice,
  HtmlGateMode,
} from "../components/HtmlGateContext.ts";

export function useHtmlGates(
  entries: TimelineEntry[],
  motionScope: string,
  allowHtml: boolean,
  atBottom: RefObject<boolean>,
) {
  // Bound choices to the current history window and conversation, surviving row unmounts.
  const { choices } = useMemo(
    () => ({
      motionScope,
      allowHtml,
      choices: new Map<string, HtmlGateChoice>(),
    }),
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
      revision: gateRevision,
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
  const { expandedMessages } = useMemo(
    () => ({ motionScope, expandedMessages: new Map<string, string>() }),
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
  return { gates, gateRevision, expandedMessages, expansion };
}
