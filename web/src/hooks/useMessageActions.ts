import type { RefObject } from "preact";
import type {
  Acknowledgement,
  Command,
  Message,
  PendingCommand,
  ReplyTarget,
} from "../api/protocol.ts";
import type { createRequestLifecycle } from "../features/conversation/requestLifecycle.ts";
import { hasMessageContent } from "../features/conversation/messageContent.ts";
import { messageActionCommand } from "../features/conversation/permissions.ts";

interface Context {
  permissions: string[];
  username: string;
  commands: Command[];
  canExecute: (name: string) => boolean;
  lifecycle: ReturnType<typeof createRequestLifecycle>;
  socket: RefObject<WebSocket | null>;
  selected: string;
  consoleView: boolean;
  replyTarget: ReplyTarget | null;
  setReplyTarget: (reply: ReplyTarget | null) => void;
  requestDelete: (action: () => void) => void;
  handleAcknowledgement: (
    data: Acknowledgement,
    current: PendingCommand,
  ) => void;
}
export function useMessageActions({
  permissions,
  username,
  commands,
  canExecute,
  lifecycle,
  socket,
  selected,
  consoleView,
  replyTarget,
  setReplyTarget,
  requestDelete,
  handleAcknowledgement,
}: Context) {
  function retract(message: Message, confirmed = false) {
    const command = messageActionCommand(
      "retract",
      message,
      username,
      permissions,
      commands,
    );
    if (
      !command ||
      lifecycle.pending ||
      socket.current?.readyState !== WebSocket.OPEN
    )
      return;
    if (!confirmed) return requestDelete(() => retract(message, true));
    sendAction(`${command} ${message.id}`);
    if (replyTarget?.id === message.id) setReplyTarget(null);
  }
  function editMessage(message: Message, value: string): Promise<boolean> {
    const command = messageActionCommand(
      "edit",
      message,
      username,
      permissions,
      commands,
    );
    if (
      !command ||
      !hasMessageContent(value) ||
      Array.from(value.trim()).length > 4000 ||
      lifecycle.pending ||
      socket.current?.readyState !== WebSocket.OPEN
    )
      return Promise.resolve(false);
    const text = `${command} ${message.id} ${value.trim()}`;
    return new Promise((resolve) => {
      lifecycle.submit({
        command: {
          text,
          room: selected,
          key: null,
          action: true,
        },
        frame: {
          room: consoleView ? null : selected,
          text,
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
  return { retract, editMessage, reactTo };
}
