import { serverBaseUrl } from "../api/server.ts";
import { api } from "../api/client.ts";
import { errorMessage } from "../api/protocol.ts";
import {
  preferenceValue,
  receivePreferences,
} from "../features/preferences/account.ts";
import { notificationManual } from "../features/preferences/notificationMode.ts";
import { useMessageActions } from "./useMessageActions.ts";
import { useCommandNavigation } from "./useCommandNavigation.ts";
import { useCommandRequests } from "./useCommandRequests.ts";
import { useDeleteConfirmation } from "./useDeleteConfirmation.ts";
import { useOutgoingMessages } from "./useOutgoingMessages.ts";
import { useCommandPreferences } from "./useCommandPreferences.ts";
import { mediaMarkdown } from "../api/media.ts";
import { hasMessageContent } from "../features/conversation/messageContent.ts";
import { useMediaUploads } from "./useMediaUploads.ts";
import type { RefObject } from "preact";
import { useMemo } from "preact/hooks";
import type {
  Snapshot,
  Room,
  PendingCommand,
  Acknowledgement,
  BotCommands,
} from "../api/protocol.ts";
import { commandText } from "../features/conversation/console.ts";
import type { ValueRef } from "./refs.ts";
import {
  expandAlias,
  browserCommands,
  aliasHelp,
  aliasManual,
} from "../features/conversation/aliases.ts";
import {
  captureSubmission,
  deletionKind,
  submissionFrame,
} from "../features/conversation/submission.ts";
import type { ComposerSubmission } from "../features/conversation/submission.ts";

import { useCommandComposer } from "./useCommandComposer.ts";

interface Context {
  setNotificationsMuted: (muted: boolean) => Promise<void>;
  openNotificationSettings: () => void;
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
  alwaysSendWithEnter: boolean;
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
  setNotificationsMuted,
  openNotificationSettings,
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
  alwaysSendWithEnter,
  desiredRoom,
  setSelected,
  append,
  complete,
  clearView,
  logout,
}: Context) {
  const preferences = useCommandPreferences(append);
  const {
    markdown,
    changeMarkdown,
    aliases,
    saveAliasMapping,
    removeAliasMapping,
  } = preferences;
  const permissions =
    room?.permissions ??
    state.private_access[selected]?.permissions ??
    (direct ? state.private_permissions : state.permissions);
  const serverCommands =
    room?.commands ??
    state.private_access[selected]?.commands ??
    (direct ? state.private_commands : state.commands);
  const availableCommands = useMemo(
    () => browserCommands(serverCommands, aliases),
    [serverCommands, aliases],
  );
  const canExecute = (name: string) =>
    serverCommands.some((command) => command.name === name);
  const navigation = useCommandNavigation({
    permissions,
    state,
    currentState,
    desiredRoom,
    setSelected,
    clearView,
    logout,
  });
  const outbox = useOutgoingMessages();
  const { lifecycle, pending, accountCommand } = useCommandRequests(socket);
  const {
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
    alwaysSendWithEnter,
    aliases,
    serverCommands,
    availableCommands,
    send,
  });
  const mediaUpload = useMediaUploads(
    room?.id ??
      (direct && peer ? `@direct:${peer}` : state.private_access[selected]?.id),
    state.sync?.instance,
  );
  const { deleteConfirmation, requestDelete, cancelDelete, confirmDelete } =
    useDeleteConfirmation();
  const { retract, editMessage, reactTo } = useMessageActions({
    permissions,
    username: state.username,
    canExecute,
    lifecycle,
    socket,
    selected,
    consoleView,
    replyTarget,
    setReplyTarget,
    requestDelete,
    handleAcknowledgement,
  });
  function send(event: Event) {
    event.preventDefault();
    if (mediaUpload.busy) return;
    const attachment = mediaUpload.attachment;
    if (attachment && !permissions.includes("w:media.upload")) return;
    const plainText = expandAlias(draft.trim(), aliases);
    if (attachment && plainText.startsWith("/")) {
      append(
        plainText,
        "Remove the attachment before running a command.",
        true,
      );
      return;
    }
    const attachmentMarkdown = attachment
      ? mediaMarkdown(attachment.media, serverBaseUrl())
      : null;
    if (attachment && !attachmentMarkdown) {
      append(
        plainText,
        "Invalid attachment address. Remove the attachment and upload it again.",
        true,
      );
      return;
    }
    const text = attachment
      ? [attachmentMarkdown, plainText].filter(Boolean).join("\n\n")
      : plainText;
    if (
      !hasMessageContent(text) ||
      (text.startsWith("/") ? lifecycle.pending : !lifecycle.canSubmitMessage())
    )
      return;
    if (/[\uD800-\uDFFF]/u.test(text)) {
      append(text, "Remove invalid Unicode characters before sending.", true);
      return;
    }
    const local = preferences.handleLocalCommand(text);
    if (local) {
      if (local === "done") setDraft("");
      return;
    }
    if (/^\/man\s+\/?notifications$/u.test(text)) {
      append(text, notificationManual);
      setDraft("");
      return;
    }
    if (/^\/notifications(?:\s|$)/u.test(text)) {
      const argument = text.slice("/notifications".length).trim();
      if (argument === "install" || argument === "uninstall") {
        openNotificationSettings();
        append(
          text,
          argument === "install"
            ? "Choose Install in Notifications settings."
            : "Choose Uninstall in Notifications settings.",
        );
      } else if (argument === "mute" || argument === "unmute") {
        const entry = append(text, null);
        setNotificationsMuted(argument === "mute")
          .then(() =>
            complete(
              entry,
              argument === "mute"
                ? "Notifications muted."
                : "Notifications unmuted.",
            ),
          )
          .catch((cause: unknown) =>
            complete(entry, errorMessage(cause), true),
          );
      } else if (argument === "status") {
        append(text, "Notification status");
      } else if (!argument) {
        const id = room?.id ?? state.private_access[selected]?.id;
        const global = preferenceValue(
          "notification.global",
          state.preferences?.["notification.global"] ?? "default",
        );
        const key = `notification.${id}`;
        const rule = id
          ? preferenceValue(key, state.preferences?.[key] ?? global)
          : global;
        append(text, `Notification filter: ${rule}.`);
      } else {
        const filter =
          /^filter\s+(.+)\s+(global|here|#[^\s]+|@direct:[^\s]+)$/u.exec(
            argument,
          );
        const scope = filter?.[2];
        const raw = filter?.[1]?.trim();
        if (!scope || !raw) {
          append(
            text,
            "Usage: /notifications filter <rules> <scope>. See /man notifications.",
            true,
          );
          return;
        }
        const view = scope === "here" ? selected : scope.replace(/^#/u, "");
        const id =
          scope === "global"
            ? "global"
            : (state.rooms.find((candidate) => candidate.name === view)?.id ??
              state.private_access[view]?.id);
        if (!id) {
          append(text, "Choose a readable room or private chat.", true);
          return;
        }
        const rule =
          raw.startsWith('"') && raw.endsWith('"') ? raw.slice(1, -1) : raw;
        const entry = append(text, null);
        api
          .preferences(
            { [`notification.${id}`]: rule === "default" ? null : rule },
            false,
          )
          .then((result) => {
            receivePreferences(result);
            complete(entry, "Notification filter saved.");
          })
          .catch((cause: unknown) =>
            complete(entry, errorMessage(cause), true),
          );
      }
      setDraft("");
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
      markdown: attachment ? true : markdown,
    });
    const kind = deletionKind(text);
    if (kind) return requestDelete(() => submitComposer(submission), kind);
    submitComposer(submission);
  }
  function submitComposer(submission: Readonly<ComposerSubmission>) {
    const { text, selected: view, replyId } = submission;
    const message = !text.startsWith("/");
    if (message ? !lifecycle.canSubmitMessage() : lifecycle.pending) return;
    const attached = message ? mediaUpload.attachment : null;
    const outgoingKey =
      message && !attached ? outbox.add(submission, replyTarget) : null;
    const submittedVersion = draftVersion.current;
    if (outgoingKey) {
      setDraft("");
      setReplyTarget(null);
    }
    dismissHints();
    lifecycle.submit({
      command: {
        text,
        room: view,
        key: text.startsWith("/") ? append(text, null, false, view) : null,
        reply: !!replyId && !text.startsWith("/"),
      },
      frame: submissionFrame(submission),
      parallel: message && !attached,
      complete: (data, current) => {
        if (attached && data.kind !== "error")
          mediaUpload.published(attached.media.media_id);
        if (outgoingKey) {
          outbox.complete(outgoingKey, data);
        } else
          handleAcknowledgement(
            data,
            current,
            submission.draft,
            submittedVersion,
          );
      },
    });
  }
  function handleAcknowledgement(
    data: Acknowledgement,
    current: PendingCommand,
    submittedDraft?: string,
    submittedVersion?: number,
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
      if (
        draftRef.current === submittedDraft &&
        draftVersion.current === submittedVersion
      )
        setDraft("");
      if (current.reply) setReplyTarget(null);
      navigation.acknowledge(current.text);
    }
  }
  return {
    mediaUpload: {
      ...mediaUpload,
      enabled: mediaUpload.enabled && permissions.includes("w:media.upload"),
    },
    markdown,
    changeMarkdown,
    aliases,
    saveAliasMapping,
    removeAliasMapping,
    accountCommand,
    permissions,
    serverCommands,
    debug: navigation.debug,
    draft,
    pending:
      pending ||
      (draft.trim().startsWith("/")
        ? lifecycle.pending
        : !lifecycle.canSubmitMessage()),
    outgoing: outbox.outgoing,
    restoreOutgoing: (key: string) => {
      if (draftRef.current) return;
      const item = outbox.take(key);
      if (!item) return;
      setSelected(item.view);
      setDraft(item.draft);
      setReplyTarget(item.reply);
    },
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
    whenIdle: lifecycle.whenIdle,
  };
}
