/** JSON contract shared by HTTP responses and WebSocket frames. */
export interface Reply {
  id: string;
  from: string;
  text: string;
  /** Zero or absent for quotes stored before sequence tracking. */
  sequence?: number;
}
export interface Message {
  edited?: boolean;
  markdown?: boolean;
  id: string;
  from: string;
  private_id?: string;
  to: string | null;
  text: string;
  time: number;
  sequence: number;
  reactions: Record<string, string[]>;
  reply: Reply | null;
  mentions: string[];
}
export interface MessageColors {
  version: string;
  values: Record<string, string | null>;
}
export interface Room {
  /** Client-only supplemental history colors; snapshots do not send this field. */
  name_colors?: MessageColors | undefined;
  id: string;
  owner: string;
  permissions: string[];
  commands: Command[];
  name: string;
  members: string[];
  messages: Message[];
  /** Room-scoped overrides of the snapshot's global `colors`. */
  colors?: Record<string, string>;
}
export interface Command {
  name: string;
  usage: string;
  description: string;
  requirements?: string;
  section: string;
  admin: boolean;
  console: boolean;
  /** Commands available inside /sudo, which can differ from direct grants. */
  subcommands?: Command[];
}
export interface BotCommand {
  name: string;
  usage: string;
  description: string;
  subcommands?: BotCommand[];
  operator_only?: boolean;
}
export interface BotCommands {
  account_id: string;
  username: string;
  commands: BotCommand[];
}
export interface BotCredentialHint {
  id: string;
  bot: string;
  label: string;
  kind: "issuer" | "worker";
}
export interface BotCommandConversation {
  conversation_id: string;
  bots: BotCommands[];
}
export interface BotCommandDirectory {
  conversations: BotCommandConversation[];
  next?: string | null;
}
export interface Unread {
  count: number;
  first: number | null;
  through: number;
  oldest: number;
  revision: number;
}
export interface Access {
  id: string;
  permissions: string[];
  commands: Command[];
}
export interface Snapshot {
  color_version?: string;
  /** Client-only supplemental history colors; snapshots do not send this field. */
  name_colors?: MessageColors | undefined;
  sync?: { instance: string; revision: number };
  kind: "snapshot";
  username: string;
  groups: string[];
  permissions: string[];
  account_access: Access;
  private_access: Record<string, Access>;
  private_permissions: string[];
  private_commands: Command[];
  policy_revision: number;
  admin: boolean;
  users: string[];
  bots?: string[];
  bot_credentials?: BotCredentialHint[];
  avatars?: Record<string, string>;
  online: string[];
  colors?: Record<string, string>;
  rooms: Room[];
  direct: Message[];
  private_peers: string[];
  commands: Command[];
  available_rooms: string[];
  unread: Record<string, Unread>;
}
export interface History {
  name_colors?: MessageColors | undefined;
  view: string;
  messages: Message[];
  revision: number;
  has_before: boolean;
  has_after: boolean;
}
export interface HistoryRange {
  around?: number;
  start?: number;
  end?: number;
  before?: number;
  after?: number;
  limit?: number;
}
export interface ConversationUpdate {
  name_colors?: MessageColors | undefined;
  kind: "conversation";
  view: string;
  id: string;
  messages: Message[];
  unread: Unread;
}
export interface AvatarUpdate {
  kind: "avatar";
  username: string;
  url: string | null;
}
export interface Acknowledgement {
  kind: "notice" | "error";
  id: number;
  text: string;
}
export interface BotActivity {
  kind: "activity";
  conversation_id: string;
  account_id: string;
  state: "working" | "idle";
  expires: number;
  draft?: string;
}
export type ServerFrame =
  | BotActivity
  | AvatarUpdate
  | { kind: "bot_commands"; conversation_id: string }
  | Snapshot
  | ConversationUpdate
  | Acknowledgement
  | { kind: "presence"; online: string[] }
  | { kind: "read"; unread: Record<string, Unread> };
export interface ClientFrame {
  markdown?: boolean;
  id: number;
  room: string | null;
  text: string;
}
export interface PendingCommand {
  id: number;
  text: string;
  room: string;
  key: string | null;
  action?: boolean;
  reply?: boolean;
}
export interface Hint {
  label: string;
  description: string;
  value: string | null;
  advance?: boolean;
}
export interface LocalOutput {
  kind: "command";
  key: string;
  order: number;
  command: string;
  result: string | null;
  error: boolean;
  room: string;
}
export interface Ledger {
  sequence: number;
  messages: Map<string, number>;
}
export type TimelineEntry =
  | LocalOutput
  | { kind: "message"; key: string; order: number; message: Message };
export interface Fonts {
  chat: number;
  ui: number;
}
export type ReplyTarget = Message & { view: string };

const record = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const strings = (v: unknown): v is string[] =>
  Array.isArray(v) && v.every((x) => typeof x === "string");
const integer = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const reply = (v: unknown): v is Reply =>
  record(v) &&
  typeof v.id === "string" &&
  typeof v.from === "string" &&
  typeof v.text === "string" &&
  (v.sequence === undefined || integer(v.sequence));
export const isMessage = (v: unknown): v is Message =>
  record(v) &&
  (v.markdown === undefined || typeof v.markdown === "boolean") &&
  (v.edited === undefined || typeof v.edited === "boolean") &&
  (v.private_id === undefined || typeof v.private_id === "string") &&
  typeof v.id === "string" &&
  typeof v.from === "string" &&
  (v.to === null || typeof v.to === "string") &&
  typeof v.text === "string" &&
  integer(v.time) &&
  v.time <= 8_640_000_000_000 &&
  integer(v.sequence) &&
  record(v.reactions) &&
  Object.values(v.reactions).every(strings) &&
  (v.reply === null || reply(v.reply)) &&
  strings(v.mentions);
const messages = (v: unknown): v is Message[] =>
  Array.isArray(v) && v.every(isMessage);
const unread = (v: unknown): v is Unread =>
  record(v) &&
  integer(v.count) &&
  (v.first === null || integer(v.first)) &&
  integer(v.through) &&
  integer(v.oldest) &&
  integer(v.revision);
const unreads = (v: unknown): v is Record<string, Unread> =>
  record(v) && Object.values(v).every(unread);
const commands = (v: unknown, depth = 0): v is Command[] =>
  Array.isArray(v) &&
  v.every(
    (c) =>
      record(c) &&
      typeof c.name === "string" &&
      typeof c.usage === "string" &&
      typeof c.description === "string" &&
      (c.requirements === undefined || typeof c.requirements === "string") &&
      typeof c.section === "string" &&
      typeof c.admin === "boolean" &&
      typeof c.console === "boolean" &&
      (c.subcommands === undefined ||
        (depth === 0 && commands(c.subcommands, depth + 1))),
  );
const colorMap = (v: unknown): v is Record<string, string> =>
  record(v) && Object.values(v).every((color) => typeof color === "string");
const messageColors = (v: unknown): v is MessageColors =>
  record(v) &&
  typeof v.version === "string" &&
  record(v.values) &&
  Object.values(v.values).every(
    (color) => color === null || typeof color === "string",
  );
export const isSnapshot = (v: unknown): v is Snapshot =>
  record(v) &&
  (v.color_version === undefined || typeof v.color_version === "string") &&
  (v.name_colors === undefined || messageColors(v.name_colors)) &&
  (v.sync === undefined ||
    (record(v.sync) &&
      typeof v.sync.instance === "string" &&
      integer(v.sync.revision))) &&
  v.kind === "snapshot" &&
  typeof v.username === "string" &&
  typeof v.admin === "boolean" &&
  strings(v.groups) &&
  strings(v.permissions) &&
  record(v.account_access) &&
  typeof v.account_access.id === "string" &&
  strings(v.account_access.permissions) &&
  commands(v.account_access.commands) &&
  strings(v.private_permissions) &&
  record(v.private_access) &&
  Object.values(v.private_access).every(
    (a) =>
      record(a) &&
      typeof a.id === "string" &&
      strings(a.permissions) &&
      commands(a.commands),
  ) &&
  commands(v.private_commands) &&
  integer(v.policy_revision) &&
  strings(v.users) &&
  (v.bots === undefined || strings(v.bots)) &&
  (v.bot_credentials === undefined ||
    (Array.isArray(v.bot_credentials) &&
      v.bot_credentials.every(
        (item: unknown) =>
          record(item) &&
          typeof item.id === "string" &&
          typeof item.bot === "string" &&
          typeof item.label === "string" &&
          (item.kind === "issuer" || item.kind === "worker"),
      ))) &&
  (v.avatars === undefined ||
    (record(v.avatars) &&
      Object.values(v.avatars).every((url) => typeof url === "string"))) &&
  strings(v.online) &&
  (v.colors === undefined || colorMap(v.colors)) &&
  Array.isArray(v.rooms) &&
  v.rooms.every(
    (r) =>
      record(r) &&
      typeof r.name === "string" &&
      typeof r.id === "string" &&
      typeof r.owner === "string" &&
      strings(r.permissions) &&
      commands(r.commands) &&
      strings(r.members) &&
      messages(r.messages) &&
      (r.colors === undefined || colorMap(r.colors)) &&
      (r.name_colors === undefined || messageColors(r.name_colors)),
  ) &&
  messages(v.direct) &&
  strings(v.private_peers) &&
  strings(v.available_rooms) &&
  unreads(v.unread) &&
  commands(v.commands);
export const isHistory = (v: unknown): v is History =>
  record(v) &&
  typeof v.view === "string" &&
  integer(v.revision) &&
  typeof v.has_before === "boolean" &&
  typeof v.has_after === "boolean" &&
  messages(v.messages) &&
  v.messages.length <= 100 &&
  (v.name_colors === undefined || messageColors(v.name_colors));
export const isOk = (v: unknown): v is { ok: true } =>
  record(v) && v.ok === true;
export function parseFrame(raw: string): ServerFrame | null {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return null;
  }
  if (isSnapshot(value)) return value;
  if (!record(value)) return null;
  if (
    value.kind === "activity" &&
    typeof value.conversation_id === "string" &&
    typeof value.account_id === "string" &&
    (value.state === "working" || value.state === "idle") &&
    integer(value.expires) &&
    (value.draft === undefined ||
      (typeof value.draft === "string" &&
        Array.from(value.draft).length <= 4000))
  ) {
    return {
      kind: "activity",
      conversation_id: value.conversation_id,
      account_id: value.account_id,
      state: value.state,
      expires: value.expires,
      ...(value.draft === undefined ? {} : { draft: value.draft }),
    };
  }
  if (
    value.kind === "bot_commands" &&
    typeof value.conversation_id === "string"
  )
    return { kind: "bot_commands", conversation_id: value.conversation_id };
  if (
    value.kind === "conversation" &&
    typeof value.view === "string" &&
    typeof value.id === "string" &&
    messages(value.messages) &&
    value.messages.length <= 50 &&
    unread(value.unread) &&
    (value.name_colors === undefined || messageColors(value.name_colors))
  )
    return {
      kind: "conversation",
      view: value.view,
      id: value.id,
      messages: value.messages,
      unread: value.unread,
      ...(value.name_colors === undefined
        ? {}
        : { name_colors: value.name_colors }),
    };
  if (
    value.kind === "avatar" &&
    typeof value.username === "string" &&
    value.username.length > 0 &&
    (value.url === null || typeof value.url === "string")
  )
    return { kind: "avatar", username: value.username, url: value.url };
  if (value.kind === "presence" && strings(value.online))
    return { kind: "presence", online: value.online };
  if (value.kind === "read" && unreads(value.unread))
    return { kind: "read", unread: value.unread };
  if (
    (value.kind === "notice" || value.kind === "error") &&
    integer(value.id) &&
    typeof value.text === "string"
  )
    return { kind: value.kind, id: value.id, text: value.text };
  return null;
}
export function errorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : "Something went wrong. Please try again.";
}
export function responseError(value: unknown): string {
  return record(value) && typeof value.error === "string"
    ? value.error
    : "Something went wrong. Please try again.";
}

function botCommandCatalogue(value: unknown): value is BotCommand[] {
  let count = 0;
  const encoder = new TextEncoder();
  function level(items: unknown, parent: string, depth: number): boolean {
    if (!Array.isArray(items) || items.length > 64) return false;
    const names = new Set<string>();
    return items.every((command: unknown) => {
      if (!record(command) || typeof command.name !== "string") return false;
      const path = parent ? `${parent} ${command.name}` : command.name;
      if (
        depth >= 3 ||
        ++count > 64 ||
        names.has(command.name) ||
        !(depth === 0 ? /^\/[a-z0-9_-]{1,32}$/ : /^[a-z0-9_-]{1,32}$/).test(
          command.name,
        ) ||
        (command.operator_only !== undefined &&
          typeof command.operator_only !== "boolean") ||
        typeof command.usage !== "string" ||
        encoder.encode(command.usage).length > 256 ||
        !(command.usage === path || command.usage.startsWith(`${path} `)) ||
        typeof command.description !== "string" ||
        !command.description.length ||
        encoder.encode(command.description).length > 512 ||
        /[\u0000-\u001f\u007f-\u009f]/u.test(
          command.usage + command.description,
        )
      )
        return false;
      names.add(command.name);
      return (
        command.subcommands === undefined ||
        level(command.subcommands, path, depth + 1)
      );
    });
  }
  return (
    level(value, "", 0) && encoder.encode(JSON.stringify(value)).length <= 32768
  );
}
export const isBotCommandDirectory = (
  value: unknown,
): value is BotCommandDirectory =>
  record(value) &&
  (value.next === undefined ||
    value.next === null ||
    typeof value.next === "string") &&
  Array.isArray(value.conversations) &&
  value.conversations.every(
    (value: unknown) =>
      record(value) &&
      typeof value.conversation_id === "string" &&
      Array.isArray(value.bots) &&
      value.bots.length <= 128 &&
      value.bots.every(
        (bot: unknown) =>
          record(bot) &&
          typeof bot.account_id === "string" &&
          typeof bot.username === "string" &&
          botCommandCatalogue(bot.commands),
      ),
  );
