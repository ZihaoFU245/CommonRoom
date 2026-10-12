import type { JSX, RefObject } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import type {
  Snapshot,
  Room,
  ReplyTarget,
  Hint,
  Command,
} from "../api/protocol.ts";
import type { Aliases } from "../features/conversation/aliases.ts";
import { expandAlias } from "../features/conversation/aliases.ts";
import { suggestions } from "../features/conversation/console.ts";
import { mentionSuggestions } from "../features/conversation/interactions.ts";

interface Context {
  selected: string;
  direct: boolean;
  peer: string | null;
  state: Snapshot;
  room: Room | undefined;
  input: RefObject<HTMLTextAreaElement>;
  pending: boolean;
  aliases: Aliases;
  serverCommands: Command[];
  availableCommands: Command[];
  send: (event: Event) => void;
}
/** Composer editing, completion, keyboard controls and reply selection. */
export function useCommandComposer({
  selected,
  direct,
  peer,
  state,
  room,
  input,
  pending,
  aliases,
  serverCommands,
  availableCommands,
  send,
}: Context) {
  const [draft, setDraft] = useState("");
  const [replyTarget, setReplyTarget] = useState<ReplyTarget | null>(null);
  const [hintIndex, setHintIndex] = useState(0);
  const [hintDismissed, setHintDismissed] = useState(false);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  useEffect(() => {
    setReplyTarget(null);
  }, [selected]);
  const hints =
    hintDismissed || pending
      ? []
      : (draft.startsWith("/")
          ? suggestions(
              /\s/u.test(draft) && !draft.startsWith("/alias ")
                ? expandAlias(draft, aliases)
                : draft,
              /\s/u.test(draft) && !draft.startsWith("/alias ")
                ? serverCommands
                : availableCommands,
              state.users,
              state.available_rooms || state.rooms.map((r) => r.name),
            )
          : mentionSuggestions(
              draft,
              direct
                ? [state.username, ...(peer ? [peer] : [])]
                : [...(room?.members || [])],
            )
        ).slice(0, 6);
  const activeHint = Math.min(hintIndex, Math.max(0, hints.length - 1));

  function edit(value: string) {
    setDraft(value);
    setHintIndex(0);
    setHintDismissed(false);
  }
  function acceptHint(hint: Hint) {
    if (!hint?.value) return;
    edit(hint.value);
    input.current?.focus();
  }
  function keydown(event: JSX.TargetedKeyboardEvent<HTMLTextAreaElement>) {
    if (event.isComposing) return;
    if (event.key === "Escape") {
      setHintDismissed(true);
      return;
    }
    if (hints.length && ["ArrowUp", "ArrowDown"].includes(event.key)) {
      event.preventDefault();
      setHintIndex(
        (activeHint + (event.key === "ArrowDown" ? 1 : hints.length - 1)) %
          hints.length,
      );
      return;
    }
    const hint = hints[activeHint];
    if (event.key === "Tab" && hint?.value) {
      event.preventDefault();
      acceptHint(hint);
      return;
    }
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      send(event);
    }
  }
  useEffect(() => {
    if (!pending) input.current?.focus();
  }, [pending]);
  return {
    draft,
    draftRef,
    setDraft,
    replyTarget,
    setReplyTarget,
    hints,
    activeHint,
    edit,
    acceptHint,
    keydown,
    dismissHints: () => setHintDismissed(true),
  };
}
