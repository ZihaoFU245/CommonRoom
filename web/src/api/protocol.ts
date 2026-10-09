/** JSON contract shared by HTTP responses and WebSocket frames. */
export interface Reply {
  id: string;
  from: string;
  text: string;
}
export interface Message {
  id: string;
  from: string;
  to: string | null;
  text: string;
  time: number;
  sequence: number;
  reactions: Record<string, string[]>;
  reply: Reply | null;
  mentions: string[];
}
export interface Room {
  name: string;
  members: string[];
  messages: Message[];
}
export interface Command {
  name: string;
  usage: string;
  description: string;
  section: string;
  admin: boolean;
  console: boolean;
}
export interface Unread {
  count: number;
  first: number | null;
  through: number;
  oldest: number;
  revision: number;
}
export interface Snapshot {
  kind: "snapshot";
  username: string;
  admin: boolean;
  users: string[];
  rooms: Room[];
  direct: Message[];
  private_peers: string[];
  commands: Command[];
  available_rooms: string[];
  unread: Record<string, Unread>;
}
export interface History {
  view: string;
  messages: Message[];
  revision: number;
}
export interface Acknowledgement {
  kind: "notice" | "error";
  id: number;
  text: string;
}
export type ServerFrame =
  Snapshot | Acknowledgement | { kind: "read"; unread: Record<string, Unread> };
export interface ClientFrame {
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
  typeof v.text === "string";
export const isMessage = (v: unknown): v is Message =>
  record(v) &&
  typeof v.id === "string" &&
  typeof v.from === "string" &&
  (v.to === null || typeof v.to === "string") &&
  typeof v.text === "string" &&
  integer(v.time) &&
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
export const isSnapshot = (v: unknown): v is Snapshot =>
  record(v) &&
  v.kind === "snapshot" &&
  typeof v.username === "string" &&
  typeof v.admin === "boolean" &&
  strings(v.users) &&
  Array.isArray(v.rooms) &&
  v.rooms.every(
    (r) =>
      record(r) &&
      typeof r.name === "string" &&
      strings(r.members) &&
      messages(r.messages),
  ) &&
  messages(v.direct) &&
  strings(v.private_peers) &&
  strings(v.available_rooms) &&
  unreads(v.unread) &&
  Array.isArray(v.commands) &&
  v.commands.every(
    (c) =>
      record(c) &&
      typeof c.name === "string" &&
      typeof c.usage === "string" &&
      typeof c.description === "string" &&
      typeof c.section === "string" &&
      typeof c.admin === "boolean" &&
      typeof c.console === "boolean",
  );
export const isHistory = (v: unknown): v is History =>
  record(v) &&
  typeof v.view === "string" &&
  integer(v.revision) &&
  messages(v.messages);
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
