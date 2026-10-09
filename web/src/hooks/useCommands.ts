import type { JSX, RefObject } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import type {
  Snapshot,
  Message,
  Room,
  ReplyTarget,
  PendingCommand,
  Hint,
  Acknowledgement,
} from "../api/protocol.ts";
import { mentionSuggestions } from "../features/conversation/interactions.ts";
import { suggestions, privatePeers } from "../features/conversation/console.ts";
import { sendFrame } from "../api/client.ts";
import type { ValueRef } from "./refs.ts";
interface Context {
  selected: string;
  direct: boolean;
  consoleView: boolean;
  peer: string | null;
  state: Snapshot;
  room: Room | undefined;
  socket: RefObject<WebSocket | null>;
  currentState: ValueRef<Snapshot>;
  input: RefObject<HTMLTextAreaElement>;
  desiredRoom: RefObject<string | null>;
  setSelected: (view: string) => void;
  append: (
    command: string,
    result?: string | null,
    error?: boolean,
    view?: string,
  ) => string;
  complete: (key: string, result: string, error?: boolean) => void;
  clearView: () => void;
  logout: () => void;
}
export function useCommands({
  selected,
  direct,
  consoleView,
  peer,
  state,
  room,
  socket,
  currentState,
  input,
  desiredRoom,
  setSelected,
  append,
  complete,
  clearView,
  logout,
}: Context) {
  const [draft, setDraft] = useState("");
  const [replyTarget, setReplyTarget] = useState<ReplyTarget | null>(null);
  const [pending, setPending] = useState(false);
  const [hintIndex, setHintIndex] = useState(0);
  const [hintDismissed, setHintDismissed] = useState(false);
  const request = useRef<PendingCommand | null>(null);
  const serial = useRef(0);
  useEffect(() => {
    setReplyTarget(null);
  }, [selected]);
  function reactTo(message: Message, value: string) {
    if (
      !value.trim() ||
      request.current ||
      socket.current?.readyState !== WebSocket.OPEN
    )
      return;
    const id = ++serial.current;
    const text = `/react ${message.id} ${value.trim()}`;
    request.current = { id, text, room: selected, key: null, action: true };
    setPending(true);
    sendFrame(socket.current, {
      id,
      room: direct || consoleView ? null : selected,
      text,
    });
  }
  const hints =
    hintDismissed || pending
      ? []
      : (draft.startsWith("/")
          ? suggestions(
              draft,
              state.commands || [],
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
  function send(event: Event) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || pending) return;
    if (!text.startsWith("/") && Array.from(text).length > 4000) {
      append(text, "Messages support at most 4000 characters.", true);
      return;
    }
    if (text === "/clear") {
      clearView();
      setDraft("");
      return;
    }
    if (text === "/logout") {
      logout();
      return;
    }
    if (socket.current?.readyState !== WebSocket.OPEN) {
      append(text, "Disconnected. Wait for the connection to recover.", true);
      return;
    }
    if (direct && !peer && !text.startsWith("/")) {
      append(text, "Use /tell user message for private messages.", true);
      return;
    }
    if (consoleView && !text.startsWith("/")) {
      append(text, "Use a command or select a room to send a message.", true);
      return;
    }
    const id = ++serial.current;
    request.current = {
      id,
      text,
      room: selected,
      key: text.startsWith("/") ? append(text) : null,
      reply: !!replyTarget && !text.startsWith("/"),
    };
    setPending(true);
    setHintDismissed(true);
    let wireText =
      peer && !text.startsWith("/") ? `/tell ${peer} ${text}` : draft;
    if (replyTarget && !text.startsWith("/"))
      wireText = `/reply ${replyTarget.id} ${draft}`;
    if (peer && /^\/history(?:\s+\d+)?$/.test(text))
      wireText = `${text === "/history" ? "/history 50" : text} ${peer}`;
    sendFrame(socket.current, {
      id,
      room: direct || consoleView ? null : selected,
      text: wireText,
    });
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
  function receive(data: Acknowledgement) {
    const current = request.current;
    if (!current || data.id !== current.id) return;
    request.current = null;
    setPending(false);
    const error = data.kind === "error";
    if (current.key) complete(current.key, data.text || "Done.", error);
    else if (error) append(current.text, data.text, true, current.room);
    if (!error && !current.action) {
      setDraft("");
      if (current.reply) setReplyTarget(null);
      const [command, target] = current.text.trim().split(/\s+/);
      if (["/new", "/join"].includes(command || "") && target) {
        desiredRoom.current = target;
        if (currentState.current.rooms.some((room) => room.name === target)) {
          setSelected(target);
          desiredRoom.current = null;
        }
      }
      if (command === "/tell" && target) {
        desiredRoom.current = `@direct:${target}`;
        if (privatePeers(currentState.current).includes(target)) {
          setSelected(desiredRoom.current);
          desiredRoom.current = null;
        }
      }
    }
  }
  function disconnect() {
    if (request.current) {
      const current = request.current;
      const error =
        "Connection lost before confirmation. Check history before sending again.";
      if (current.key) complete(current.key, error, true);
      else append(current.text, error, true, current.room);
      request.current = null;
      setPending(false);
    }
  }
  return {
    draft,
    pending,
    replyTarget,
    setReplyTarget,
    hints,
    activeHint,
    send,
    edit,
    keydown,
    acceptHint,
    reactTo,
    receive,
    disconnect,
  };
}
