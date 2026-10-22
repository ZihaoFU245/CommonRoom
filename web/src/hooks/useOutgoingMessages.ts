import { useRef, useState } from "preact/hooks";
import type { Acknowledgement, ReplyTarget } from "../api/protocol.ts";
import type { ComposerSubmission } from "../features/conversation/submission.ts";

interface Outgoing {
  key: string;
  view: string;
  draft: string;
  text: string;
  reply: ReplyTarget | null;
  error?: string;
}

export function useOutgoingMessages() {
  const [outgoing, setOutgoing] = useState<Outgoing[]>([]);
  const serial = useRef(0);
  return {
    outgoing,
    add(submission: Readonly<ComposerSubmission>, reply: ReplyTarget | null) {
      const key = `outgoing-${++serial.current}`;
      setOutgoing((items) => [
        ...items.filter(
          (item, index) => !item.error || index >= items.length - 48,
        ),
        {
          key,
          view: submission.selected,
          draft: submission.draft,
          text: submission.text,
          reply,
        },
      ]);
      return key;
    },
    complete(key: string, result: Acknowledgement) {
      setOutgoing((items) =>
        result.kind === "error"
          ? items.map((item) =>
              item.key === key ? { ...item, error: result.text } : item,
            )
          : items.filter((item) => item.key !== key),
      );
    },
    take(key: string) {
      const item = outgoing.find((entry) => entry.key === key);
      if (!item?.error) return undefined;
      setOutgoing((items) => items.filter((entry) => entry.key !== key));
      return item;
    },
  };
}
