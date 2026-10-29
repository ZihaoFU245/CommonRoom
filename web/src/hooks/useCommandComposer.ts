import type { JSX, RefObject } from "preact";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "preact/hooks";
import type {
  Snapshot,
  Room,
  ReplyTarget,
  Hint,
  Command,
  BotCommands,
} from "../api/protocol.ts";
import type { Aliases } from "../features/conversation/aliases.ts";
import { expandAlias } from "../features/conversation/aliases.ts";
import { suggestions } from "../features/conversation/console.ts";
import { mentionSuggestions } from "../features/conversation/interactions.ts";
import { botCommandSuggestions } from "../features/conversation/botCommands.ts";
import {
  cursorSuggestions,
  nextHint,
} from "../features/conversation/completion.ts";

interface Context {
  botCommands: BotCommands[];
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
  botCommands,
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
  const [draft, updateDraft] = useState("");
  const draftVersion = useRef(0);
  const [replyTarget, setReplyTarget] = useState<ReplyTarget | null>(null);
  const [hintIndex, setHintIndex] = useState(0);
  const [hintDismissed, setHintDismissed] = useState(false);
  const [caret, setCaret] = useState(0);
  const [cycle, setCycle] = useState<{ hints: Hint[]; suffix: string } | null>(
    null,
  );
  const expectedCaret = useRef<number | null>(null);
  const lastCompletedIndex = useRef<number | null>(null);
  useLayoutEffect(() => {
    if (
      expectedCaret.current !== null &&
      document.activeElement === input.current
    )
      input.current?.setSelectionRange(
        expectedCaret.current,
        expectedCaret.current,
      );
  }, [draft, caret]);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  function setDraft(value: string) {
    draftVersion.current++;
    draftRef.current = value;
    updateDraft(value);
  }
  useEffect(() => {
    if (pending || !draft) {
      setCycle(null);
      setHintIndex(0);
      expectedCaret.current = null;
      lastCompletedIndex.current = null;
    }
  }, [pending, draft]);
  useEffect(() => {
    setReplyTarget(null);
    setCycle(null);
    setHintIndex(0);
  }, [selected]);
  useEffect(() => {
    setCycle(null);
    setHintIndex(0);
  }, [botCommands, state.bot_credentials]);
  const completion = useMemo(
    () =>
      cursorSuggestions(draft, caret, (prefix) =>
        prefix.startsWith("/")
          ? suggestions(
              /\s/u.test(prefix) && !prefix.startsWith("/alias ")
                ? expandAlias(prefix, aliases)
                : prefix,
              /\s/u.test(prefix) && !prefix.startsWith("/alias ")
                ? serverCommands
                : availableCommands,
              state.users,
              state.available_rooms || state.rooms.map((r) => r.name),
              {
                bots: state.bots ?? [],
                botCredentials: state.bot_credentials ?? [],
                messages: direct
                  ? state.direct.filter(
                      (message) =>
                        (message.from === state.username &&
                          message.to === peer) ||
                        (message.from === peer &&
                          message.to === state.username),
                    )
                  : room?.messages || [],
                aliases: Object.keys(aliases),
                privateTargets: Object.keys(state.private_access),
                privateScopes: Object.keys(state.private_access).map((view) =>
                  view.startsWith("@direct:")
                    ? `@private:${[state.username, view.slice(8)].sort().join(":")}`
                    : view,
                ),
              },
            )
          : [
              ...botCommandSuggestions(prefix, botCommands),
              ...mentionSuggestions(
                prefix,
                direct
                  ? [state.username, ...(peer ? [peer] : [])]
                  : [...(room?.members || [])],
              ),
            ],
      ),
    [
      draft,
      caret,
      aliases,
      serverCommands,
      availableCommands,
      state.users,
      state.bots,
      state.bot_credentials,
      state.available_rooms,
      state.rooms,
      state.direct,
      state.private_access,
      state.username,
      direct,
      peer,
      room,
      botCommands,
    ],
  );
  const currentCompletion = cycle || completion;
  const hints = hintDismissed ? [] : currentCompletion.hints;
  const activeHint = Math.min(hintIndex, Math.max(0, hints.length - 1));

  function edit(value: string) {
    setDraft(value);
    setCaret(input.current?.selectionStart ?? value.length);
    setCycle(null);
    expectedCaret.current = null;
    lastCompletedIndex.current = null;
    setHintIndex(0);
    setHintDismissed(false);
  }
  function acceptHint(hint: Hint) {
    if (!hint?.value) return;
    setCycle(null);
    applyHint(hint);
    setHintIndex(0);
  }
  function applyHint(hint: Hint) {
    if (!hint.value) return;
    const position = hint.value.length - currentCompletion.suffix.length;
    expectedCaret.current = position;
    input.current?.focus();
    setDraft(hint.value);
    setCaret(position);
  }
  function selectCaret() {
    const position = input.current?.selectionStart ?? draft.length;
    if (expectedCaret.current === position || position === caret) return;
    expectedCaret.current = null;
    setCaret(position);
    setCycle(null);
    setHintIndex(0);
    setHintDismissed(false);
  }
  function keydown(event: JSX.TargetedKeyboardEvent<HTMLTextAreaElement>) {
    if (event.isComposing) return;
    if (event.key === "Escape") {
      setHintDismissed(true);
      setCycle(null);
      return;
    }
    if (hints.length && ["ArrowUp", "ArrowDown"].includes(event.key)) {
      event.preventDefault();
      lastCompletedIndex.current = null;
      setHintIndex(nextHint(activeHint, hints.length, event.key === "ArrowUp"));
      return;
    }
    const hint = hints[activeHint];
    if (event.key === "Tab" && event.shiftKey) return;
    if (event.key === "Tab" && hint?.value) {
      event.preventDefault();
      const index =
        cycle && lastCompletedIndex.current === activeHint
          ? nextHint(activeHint, hints.length)
          : activeHint;
      const candidate = hints[index];
      if (candidate) {
        if (candidate.advance) {
          setCycle(null);
          setHintIndex(0);
          lastCompletedIndex.current = null;
        } else {
          if (hints.length > 1) setCycle(currentCompletion);
          setHintIndex(index);
          lastCompletedIndex.current = index;
        }
        applyHint(candidate);
      }
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
    draftVersion,
    setDraft,
    replyTarget,
    setReplyTarget,
    hints,
    activeHint,
    edit,
    acceptHint,
    keydown,
    selectCaret,
    dismissHints: () => setHintDismissed(true),
  };
}
