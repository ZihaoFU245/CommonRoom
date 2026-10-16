import {
  loadMarkdown,
  setMarkdown,
  saveMarkdown,
  markdownManual,
} from "../features/preferences/markdown.ts";
import type { RefObject } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import type {
  Snapshot,
  Message,
  Room,
  PendingCommand,
  Acknowledgement,
  BotCommands,
} from "../api/protocol.ts";
import { privatePeers, commandText } from "../features/conversation/console.ts";
import { sendFrame } from "../api/client.ts";
import type { ValueRef } from "./refs.ts";
import {
  loadAliases,
  saveAlias,
  expandAlias,
  browserCommands,
  aliasHelp,
  aliasManual,
  setAlias,
  removeAlias,
} from "../features/conversation/aliases.ts";
import { createRequestLifecycle } from "../features/conversation/requestLifecycle.ts";
import { debugAccess } from "../features/conversation/permissions.ts";
import {
  captureSubmission,
  deletionKind,
  submissionFrame,
} from "../features/conversation/submission.ts";
import type { ComposerSubmission } from "../features/conversation/submission.ts";

import { useCommandComposer } from "./useCommandComposer.ts";

interface Context {
  botCommands: BotCommands[];
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
  botCommands,
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
  const [markdown, updateMarkdown] = useState(loadMarkdown);
  function changeMarkdown(enabled: boolean) {
    updateMarkdown(saveMarkdown(enabled));
  }
  const [aliases, setAliases] = useState(loadAliases);
  function saveAliasMapping(name: string, target: string) {
    setAliases(setAlias(aliases, name, target));
  }
  function removeAliasMapping(name: string) {
    setAliases(removeAlias(aliases, name));
  }
  const permissions =
    room?.permissions ??
    state.private_access[selected]?.permissions ??
    (direct ? state.private_permissions : state.permissions);
  const serverCommands =
    room?.commands ??
    state.private_access[selected]?.commands ??
    (direct ? state.private_commands : state.commands);
  const availableCommands = browserCommands(serverCommands, aliases);
  const canExecute = (name: string) =>
    serverCommands.some((command) => command.name === name);
  const [debug, setDebug] = useState(false);
  const [sudoDebug, setSudoDebug] = useState(false);
  const canDebug = debugAccess(
    permissions,
    sudoDebug,
    state.permissions,
    state.commands,
  );
  useEffect(() => {
    if (!canDebug) {
      setDebug(false);
      setSudoDebug(false);
    }
  }, [canDebug]);
  const [pending, setPending] = useState(false);
  const {
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
    selectCaret,
    dismissHints,
  } = useCommandComposer({
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
  });
  const lifecycleRef = useRef<ReturnType<typeof createRequestLifecycle> | null>(
    null,
  );
  if (!lifecycleRef.current)
    lifecycleRef.current = createRequestLifecycle((frame) => {
      const connection = socket.current;
      if (connection?.readyState !== WebSocket.OPEN)
        throw new Error("Disconnected. Wait for the connection to recover.");
      sendFrame(connection, frame);
    }, setPending);
  const lifecycle = lifecycleRef.current;
  function accountCommand(text: string): Promise<Acknowledgement> {
    if (lifecycle.pending)
      return Promise.reject(new Error("Another command is pending."));
    return new Promise((resolve) => {
      lifecycle.submit({
        command: { text, room: "@command", key: null, action: true },
        frame: { room: null, text },
        complete: resolve,
      });
    });
  }
  const [deleteConfirmation, setDeleteConfirmation] = useState<
    "message" | "resource" | null
  >(null);
  const deleteAction = useRef<(() => void) | null>(null);
  function requestDelete(
    action: () => void,
    kind: "message" | "resource" = "message",
  ) {
    deleteAction.current = action;
    setDeleteConfirmation(kind);
  }
  function cancelDelete() {
    deleteAction.current = null;
    setDeleteConfirmation(null);
  }
  function confirmDelete() {
    const action = deleteAction.current;
    cancelDelete();
    action?.();
  }
  function retract(message: Message, confirmed = false) {
    if (
      (!permissions.includes("w:message.retract.any") &&
        (message.from !== state.username ||
          !permissions.includes("w:message.retract.own"))) ||
      !canExecute("/retract") ||
      lifecycle.pending ||
      socket.current?.readyState !== WebSocket.OPEN
    )
      return;
    if (!confirmed) return requestDelete(() => retract(message, true));
    sendAction(`/retract ${message.id}`);
    if (replyTarget?.id === message.id) setReplyTarget(null);
  }
  function editMessage(message: Message, value: string): Promise<boolean> {
    if (
      (!permissions.includes("w:message.edit.any") &&
        (message.from !== state.username ||
          !permissions.includes("w:message.edit.own"))) ||
      !canExecute("/edit") ||
      !value.trim() ||
      Array.from(value.trim()).length > 4000 ||
      lifecycle.pending ||
      socket.current?.readyState !== WebSocket.OPEN
    )
      return Promise.resolve(false);
    return new Promise((resolve) => {
      lifecycle.submit({
        command: {
          text: `/edit ${message.id} ${value.trim()}`,
          room: selected,
          key: null,
          action: true,
        },
        frame: {
          room: consoleView ? null : selected,
          text: `/edit ${message.id} ${value.trim()}`,
        },
        complete: (result, current) => {
          handleAcknowledgement(result, current);
          resolve(result.kind !== "error");
        },
      });
    });
  }
  function reactTo(message: Message, value: string) {
    if (
      !permissions.includes("w:message.react") ||
      !canExecute("/react") ||
      !value.trim() ||
      lifecycle.pending ||
      socket.current?.readyState !== WebSocket.OPEN
    )
      return;
    sendAction(`/react ${message.id} ${value.trim()}`);
  }
  function sendAction(text: string) {
    lifecycle.submit({
      command: { text, room: selected, key: null, action: true },
      frame: { room: consoleView ? null : selected, text },
      complete: handleAcknowledgement,
    });
  }
  function send(event: Event) {
    event.preventDefault();
    const text = expandAlias(draft.trim(), aliases);
    if (!text || lifecycle.pending) return;
    if (text.split(/\s+/u)[0] === "/markdown") {
      try {
        const enabled = setMarkdown(text.replace(/\s+/gu, " "));
        updateMarkdown(enabled);
        append(text, `Markdown ${enabled ? "on" : "off"} for future messages.`);
        setDraft("");
      } catch (error: unknown) {
        append(
          text,
          error instanceof Error ? error.message : "Could not update Markdown.",
          true,
        );
      }
      return;
    }
    if (/^\/man\s+\/?markdown$/u.test(text)) {
      append(text, markdownManual);
      setDraft("");
      return;
    }
    if (text.split(/\s+/u)[0] === "/alias") {
      try {
        setAliases(saveAlias(aliases, text));
        append(text, "Alias saved in this browser.");
        setDraft("");
      } catch (error: unknown) {
        append(
          text,
          error instanceof Error ? error.message : "Alias could not be saved.",
          true,
        );
      }
      return;
    }
    if (!text.startsWith("/") && Array.from(text).length > 4000) {
      append(text, "Messages support at most 4000 characters.", true);
      return;
    }
    if (!text.startsWith("/") && !permissions.includes("w:message.create")) {
      append(text, "This conversation is read-only.", true);
      return;
    }
    if (text.startsWith("/") && !canExecute(text.split(/\s+/)[0] ?? "")) {
      const command = text.split(/\s+/u)[0] ?? "";
      const scope = consoleView ? "Command view" : selected;
      append(
        text,
        `Command grant missing: ${command} in ${scope}. Action permissions alone do not enable a command. Check /permissions or /man ${command.slice(1)}.`,
        true,
      );
      return;
    }
    if (text === "/clear") {
      clearView();
      setDraft("");
      return;
    }
    if (/^\/man\s+\/?alias$/u.test(text)) {
      append(text, aliasManual);
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
    const submission = captureSubmission({
      text,
      draft,
      selected,
      consoleView,
      peer,
      replyId: replyTarget?.id ?? null,
      markdown,
    });
    const kind = deletionKind(text);
    if (kind) return requestDelete(() => submitComposer(submission), kind);
    submitComposer(submission);
  }
  function submitComposer(submission: Readonly<ComposerSubmission>) {
    if (lifecycle.pending) return;
    const { text, selected: view, replyId } = submission;
    dismissHints();
    lifecycle.submit({
      command: {
        text,
        room: view,
        key: text.startsWith("/") ? append(text, null, false, view) : null,
        reply: !!replyId && !text.startsWith("/"),
      },
      frame: submissionFrame(submission),
      complete: (data, current) =>
        handleAcknowledgement(data, current, submission.draft),
    });
  }
  function handleAcknowledgement(
    data: Acknowledgement,
    current: PendingCommand,
    submittedDraft?: string,
  ) {
    const error = data.kind === "error";
    if (current.key) {
      const localHelp =
        !error && commandText(current.text) === "/help"
          ? `\n\n${aliasHelp(aliases)}`
          : "";
      complete(current.key, (data.text || "Done.") + localHelp, error);
    } else if (error) append(current.text, data.text, true, current.room);
    if (!error && !current.action) {
      if (draftRef.current === submittedDraft) setDraft("");
      if (current.reply) setReplyTarget(null);
      const [command, argument, name] = commandText(current.text).split(/\s+/u);
      const target =
        command === "/new"
          ? argument === "room"
            ? name
            : undefined
          : argument;
      if (command === "/clear") clearView();
      if (command === "/logout") logout();
      if (command === "/console") {
        desiredRoom.current = null;
        setSelected("@command");
      }
      if (["/new", "/join"].includes(command || "") && target) {
        desiredRoom.current = target;
        if (currentState.current.rooms.some((room) => room.name === target)) {
          setSelected(target);
          desiredRoom.current = null;
        }
      }
      if (command === "/debug") {
        setDebug(target === "on");
        setSudoDebug(
          target === "on" && current.text.trim().split(/\s+/u)[0] === "/sudo",
        );
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
  return {
    markdown,
    changeMarkdown,
    aliases,
    saveAliasMapping,
    removeAliasMapping,
    accountCommand,
    permissions,
    serverCommands,
    debug: debug && canDebug,
    draft,
    pending,
    replyTarget,
    setReplyTarget,
    hints,
    activeHint,
    send,
    edit,
    keydown,
    selectCaret,
    acceptHint,
    reactTo,
    editMessage,
    retract,
    deleteConfirmation,
    cancelDelete,
    confirmDelete,
    receive: lifecycle.receive,
    disconnect: lifecycle.disconnect,
  };
}
