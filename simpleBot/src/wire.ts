// Wire types for CommonRoom Bot API v1, transcribed from docs/bot-v1.schema.json.
//
// Keep this file aligned with the published schema. `GET /api/bot/v1/schema`
// serves it. Runtime guards below validate payloads before the agent sees them.

export type ErrorCode =
  | "invalid_request"
  | "unauthorized"
  | "forbidden"
  | "not_found"
  | "method_not_allowed"
  | "conflict"
  | "idempotency_conflict"
  | "message_gone"
  | "payload_too_large"
  | "rate_limited"
  | "idempotency_capacity"
  | "runtime_rejected"
  | "unavailable";

export interface ApiErrorBody {
  code: ErrorCode;
  message: string;
  retryable: boolean;
}

export interface Permissions {
  send: boolean;
  reply: boolean;
  react: boolean;
}

export interface Unread {
  count: number;
  first: number | null;
  through: number;
  oldest: number;
  revision: number;
}

export interface Conversation {
  id: string;
  view: string;
  permissions: Permissions;
  members: string[];
  unread: Unread | null;
  content_revision: number;
}

export interface CredentialContext {
  id: string;
  expires: number | null;
  conversations: string[] | null;
  operations: string[] | null;
}

export interface Snapshot {
  kind: "snapshot";
  account: { id: string; username: string };
  conversations: Conversation[];
  credential: CredentialContext;
}

export interface Reply {
  id: string;
  from: string;
  text: string;
}

export interface Message {
  id: string;
  from: string;
  author_id: string;
  text: string;
  markdown: boolean;
  edited: boolean;
  time: number;
  sequence: number;
  reply: Reply | null;
  mentions: string[];
  reactions: Record<string, string[]>;
}

export interface MessageRequest {
  conversation_id: string;
  text: string;
  reply_to?: string | null;
  markdown?: boolean;
}

export interface SendResponse {
  message: Message;
  replayed: boolean;
}

export interface HistoryResponse {
  conversation_id: string;
  messages: Message[];
  content_revision: number;
  has_before: boolean;
  has_after: boolean;
}

/** Cursors accepted by `GET /history`. `before`/`after` are exclusive. */
export interface HistoryQuery {
  message_id?: string;
  conversation_id: string;
  start?: number;
  end?: number;
  before?: number;
  after?: number;
  around?: number;
  limit?: number;
  content_revision?: number;
}

export interface CredentialRequest {
  label: string;
  expires: number;
  conversations: string[];
  operations: string[];
}

export type ConversationRequest =
  { kind: "room"; name: string } | { kind: "direct"; username: string };

/** Server → client frames. */
export type ServerFrame =
  | { kind: "commands_request"; id: string; conversation_id: string }
  | Snapshot
  | { kind: "conversation"; conversation_id: string; content_revision: number }
  | { kind: "presence"; online: string[] }
  | {
      kind: "activity";
      conversation_id: string;
      state: "working" | "idle";
      account_id: string;
      expires: number;
    }
  | {
      kind: "config_request";
      id: string;
      operation: "set" | "fetch";
      key: string | null;
      value: string | null;
    }
  | { kind: "config_ack"; id: string | null }
  | { kind: "error"; id: string | null; error: ApiErrorBody };

export type ActivityState = "working" | "idle";

/** Client → server frames accepted by the bot socket. */
export type ClientFrame =
  | { kind: "commands_ready" }
  | { kind: "commands_response"; id: string; commands: BotCommand[] }
  | { kind: "commands_changed" }
  | { kind: "config_ready" }
  | {
      kind: "config_response";
      id: string;
      values: Record<string, string> | null;
      error: string | null;
    }
  | { kind: "activity"; conversation_id: string; state: ActivityState };

export interface BotCommand {
  name: string;
  usage: string;
  description: string;
  subcommands?: BotCommand[];
  operator_only?: boolean;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function strings(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.every((item: unknown) => typeof item === "string")
  );
}
function natural(value: unknown): boolean {
  return Number.isSafeInteger(value) && Number(value) >= 0;
}
function nullableString(value: unknown): boolean {
  return value === null || typeof value === "string";
}
export function isApiError(value: unknown): value is ApiErrorBody {
  return (
    record(value) &&
    typeof value.code === "string" &&
    typeof value.message === "string" &&
    typeof value.retryable === "boolean"
  );
}
export function isMessage(value: unknown): value is Message {
  return (
    record(value) &&
    typeof value.id === "string" &&
    typeof value.author_id === "string" &&
    typeof value.from === "string" &&
    typeof value.text === "string" &&
    typeof value.markdown === "boolean" &&
    typeof value.edited === "boolean" &&
    natural(value.time) &&
    natural(value.sequence) &&
    strings(value.mentions) &&
    record(value.reactions) &&
    Object.values(value.reactions).every(strings) &&
    (value.reply === null ||
      (record(value.reply) &&
        typeof value.reply.id === "string" &&
        typeof value.reply.from === "string" &&
        typeof value.reply.text === "string"))
  );
}
export function isSnapshot(value: unknown): value is Snapshot {
  if (
    !record(value) ||
    value.kind !== "snapshot" ||
    !record(value.account) ||
    typeof value.account.id !== "string" ||
    typeof value.account.username !== "string" ||
    !record(value.credential) ||
    typeof value.credential.id !== "string" ||
    !(value.credential.expires === null || natural(value.credential.expires)) ||
    !(
      value.credential.operations === null ||
      strings(value.credential.operations)
    ) ||
    !(
      value.credential.conversations === null ||
      strings(value.credential.conversations)
    ) ||
    !Array.isArray(value.conversations)
  )
    return false;
  return value.conversations.every(
    (c: unknown) =>
      record(c) &&
      typeof c.id === "string" &&
      typeof c.view === "string" &&
      strings(c.members) &&
      natural(c.content_revision) &&
      record(c.permissions) &&
      typeof c.permissions.send === "boolean" &&
      typeof c.permissions.reply === "boolean" &&
      typeof c.permissions.react === "boolean" &&
      (c.unread === null ||
        (record(c.unread) &&
          natural(c.unread.count) &&
          natural(c.unread.through) &&
          natural(c.unread.oldest) &&
          natural(c.unread.revision) &&
          (c.unread.first === null || natural(c.unread.first)))),
  );
}
export function isHistoryResponse(value: unknown): value is HistoryResponse {
  return (
    record(value) &&
    typeof value.conversation_id === "string" &&
    natural(value.content_revision) &&
    Array.isArray(value.messages) &&
    value.messages.every(isMessage) &&
    typeof value.has_before === "boolean" &&
    typeof value.has_after === "boolean"
  );
}
export function isSendResponse(value: unknown): value is SendResponse {
  return (
    record(value) &&
    isMessage(value.message) &&
    typeof value.replayed === "boolean"
  );
}

/** Validate untrusted socket frames before dispatching callbacks. */
export function parseServerFrame(value: unknown): ServerFrame | null {
  if (!record(value)) return null;
  let valid = false;
  switch (value.kind) {
    case "commands_request":
      valid =
        typeof value.id === "string" &&
        typeof value.conversation_id === "string";
      break;
    case "snapshot":
      return isSnapshot(value) ? value : null;
    case "conversation":
      valid =
        typeof value.conversation_id === "string" &&
        natural(value.content_revision);
      break;
    case "presence":
      valid = strings(value.online);
      break;
    case "activity":
      valid =
        typeof value.conversation_id === "string" &&
        typeof value.account_id === "string" &&
        (value.state === "working" || value.state === "idle") &&
        natural(value.expires);
      break;
    case "config_request":
      valid =
        typeof value.id === "string" &&
        (value.operation === "set" || value.operation === "fetch") &&
        nullableString(value.key) &&
        nullableString(value.value);
      break;
    case "config_ack":
      valid = nullableString(value.id);
      break;
    case "error":
      valid = nullableString(value.id) && isApiError(value.error);
      break;
  }
  return valid ? (value as ServerFrame) : null;
}

export function isServerCommands(value: unknown): boolean {
  return (
    record(value) &&
    typeof value.conversation_id === "string" &&
    Array.isArray(value.commands) &&
    value.commands.length <= 64 &&
    value.commands.every(
      (command: unknown) =>
        record(command) &&
        typeof command.name === "string" &&
        /^\/[a-z0-9_-]{1,32}$/.test(command.name) &&
        typeof command.usage === "string" &&
        typeof command.description === "string",
    )
  );
}
export function isCommandResult(value: unknown): boolean {
  return (
    record(value) &&
    typeof value.result === "string" &&
    typeof value.replayed === "boolean" &&
    Number.isSafeInteger(value.expires)
  );
}

export function isMessageContext(value: unknown): boolean {
  return (
    record(value) &&
    record(value.author) &&
    typeof value.author.id === "string" &&
    typeof value.author.username === "string" &&
    strings(value.author.groups) &&
    (value.author.permissions === null || strings(value.author.permissions)) &&
    record(value.bot) &&
    typeof value.bot.id === "string" &&
    strings(value.bot.groups) &&
    (value.bot.owner_id === null || typeof value.bot.owner_id === "string") &&
    (value.bot.owner === null || typeof value.bot.owner === "string")
  );
}
