import { MAX_GROUP_RESPONSE_CHARS } from "./group.ts";

export const STATE_VERSION = 1;

export interface OutboxPart {
  slot: string;
  key: string;
  request: {
    conversation_id: string;
    text: string;
    reply_to?: string;
    markdown?: boolean;
  };
  created: number;
  response?: { message: { id: string; sequence: number }; replayed: boolean };
}

export interface CommandAction {
  command: string;
  status: "pending" | "done";
  key?: string;
  created?: number;
  result?: string;
}

export interface TaskRecord {
  conversation: string;
  input: string;
  /** Global sequence of the input message, when it was first observed. */
  sequence: number;
  promptHash: string;
  replyTo: string | null;
  status: "queued" | "running" | "answered" | "cancelled" | "failed";
  attempts: number;
  privateCommand?: boolean;
  /** Fixed-window group inputs; only the leader runs the model. */
  autonomous?: boolean;
  readyAt?: number;
  inputChars?: number;
  batchOwner?: string;
  batch?: { input: string; sequence: number; hash: string }[];
  /** Fully persisted decision, including silence, before any batch output. */
  decision?: { text: string; reply_to?: string }[];
  batchStarted?: number;
  actions?: CommandAction[];
  slots: string[];
  /** Durable output family for the version currently being answered. */
  outputHash?: string;
  /** Persist a completed decision to send no reply for this input version. */
  silentHash?: string;
  outputSlot?: string;
  pass?: number;
  error?: string;
  updated: number;
}

export interface ConversationMemory {
  /** Map of message ID to a text hash of the version we accepted. */
  known: Record<string, string>;
  /** Highest sequence observed, used for the "history gap" warning. */
  sequence: number;
  /** Highest sequence this worker has actually scanned, trimmed or not. */
  latest: number;
}

export interface State {
  version: number;
  account: string | null;
  cursors: Record<string, number>;
  revisions: Record<string, number>;
  memory: Record<string, ConversationMemory>;
  tasks: Record<string, TaskRecord>;
  parts: Record<string, OutboxPart>;
}

export interface Metrics {
  syncs: number;
  frames: number;
  reconnects: number;
  sent: number;
  replayed: number;
  revisions: number;
  edits: number;
  retractions: number;
}

export function taskKey(conversationId: string, messageId: string): string {
  return `${conversationId}:${messageId}`;
}

export function textHash(text: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index++) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash.toString(16).padStart(8, "0");
}

export function emptyState(): State {
  return {
    version: STATE_VERSION,
    account: null,
    cursors: Object.create(null) as Record<string, number>,
    revisions: Object.create(null) as Record<string, number>,
    memory: Object.create(null) as Record<string, ConversationMemory>,
    tasks: Object.create(null) as Record<string, TaskRecord>,
    parts: Object.create(null) as Record<string, OutboxPart>,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Reject a checkpoint we do not understand instead of guessing. */
export function validate(value: unknown): State {
  if (!isRecord(value)) throw new Error("checkpoint is not an object");
  if (value.version !== STATE_VERSION) {
    throw new Error(
      `checkpoint version ${String(value.version)} is not supported (expected ${STATE_VERSION}); ` +
        "reconcile the old file deliberately before replacing it",
    );
  }
  for (const key of ["cursors", "revisions", "memory", "tasks", "parts"]) {
    if (!isRecord(value[key]))
      throw new Error(`checkpoint field ${key} is invalid`);
  }
  const cursors = value.cursors as Record<string, unknown>;
  for (const [id, cursor] of Object.entries(cursors)) {
    if (!Number.isSafeInteger(cursor) || (cursor as number) < 0) {
      throw new Error(`checkpoint cursor for ${id} is invalid`);
    }
  }
  const state = value as unknown as State;
  state.version = STATE_VERSION;
  if (value.account !== null && typeof value.account !== "string")
    throw new Error("checkpoint account is invalid");
  if (
    !Object.values(state.revisions).every(
      (revision) => Number.isSafeInteger(revision) && revision >= 0,
    )
  ) {
    throw new Error("checkpoint revisions are invalid");
  }
  for (const memory of Object.values(state.memory)) {
    if (
      !isRecord(memory) ||
      !isRecord(memory.known) ||
      !Object.values(memory.known).every((hash) => typeof hash === "string") ||
      !Number.isSafeInteger(memory.latest) ||
      memory.latest < 0 ||
      !Number.isSafeInteger(memory.sequence) ||
      memory.sequence < 0
    ) {
      throw new Error("checkpoint memory is invalid");
    }
  }
  for (const task of Object.values(state.tasks)) {
    if (
      !isRecord(task) ||
      typeof task.conversation !== "string" ||
      typeof task.input !== "string" ||
      !Number.isSafeInteger(task.sequence) ||
      task.sequence < 1 ||
      typeof task.promptHash !== "string" ||
      (task.silentHash !== undefined && typeof task.silentHash !== "string") ||
      (task.autonomous !== undefined && typeof task.autonomous !== "boolean") ||
      (task.inputChars !== undefined &&
        (!Number.isSafeInteger(task.inputChars) ||
          task.inputChars < 0 ||
          task.inputChars > 8000)) ||
      (task.readyAt !== undefined && !Number.isFinite(task.readyAt)) ||
      (task.batchStarted !== undefined &&
        !Number.isFinite(task.batchStarted)) ||
      (task.batchOwner !== undefined && typeof task.batchOwner !== "string") ||
      (task.batch !== undefined &&
        (!Array.isArray(task.batch) ||
          task.batch.length < 1 ||
          task.batch.length > 100 ||
          !task.batch.every(
            (input: unknown) =>
              isRecord(input) &&
              typeof input.input === "string" &&
              typeof input.hash === "string" &&
              Number.isSafeInteger(input.sequence) &&
              Number(input.sequence) > 0,
          ))) ||
      (task.decision !== undefined &&
        (!Array.isArray(task.decision) ||
          task.decision.length > 16 ||
          !task.decision.every(
            (response: unknown) =>
              isRecord(response) &&
              typeof response.text === "string" &&
              response.text.length <= MAX_GROUP_RESPONSE_CHARS &&
              (response.reply_to === undefined ||
                typeof response.reply_to === "string"),
          ))) ||
      (task.privateCommand !== undefined &&
        typeof task.privateCommand !== "boolean") ||
      (task.actions !== undefined &&
        (!Array.isArray(task.actions) ||
          task.actions.length > 16 ||
          !task.actions.every(
            (action: unknown) =>
              isRecord(action) &&
              typeof action.command === "string" &&
              action.command.length <= 4100 &&
              ["pending", "done"].includes(String(action.status)) &&
              (action.key === undefined ||
                (typeof action.key === "string" &&
                  /^[A-Za-z0-9._:-]{1,128}$/.test(action.key) &&
                  Number.isFinite(action.created))) &&
              (action.result === undefined ||
                (typeof action.result === "string" &&
                  action.result.length <= 8000)),
          ))) ||
      (task.replyTo !== null && typeof task.replyTo !== "string") ||
      !["queued", "running", "answered", "failed", "cancelled"].includes(
        task.status,
      ) ||
      !Array.isArray(task.slots) ||
      !task.slots.every((slot: unknown) => typeof slot === "string") ||
      !Number.isSafeInteger(task.attempts) ||
      task.attempts < 0 ||
      !Number.isFinite(task.updated)
    ) {
      throw new Error("checkpoint task is invalid");
    }
  }
  for (const part of Object.values(state.parts)) {
    if (
      !isRecord(part) ||
      typeof part.slot !== "string" ||
      typeof part.key !== "string" ||
      !/^[A-Za-z0-9._:-]{1,128}$/.test(part.key) ||
      !Number.isFinite(part.created) ||
      !isRecord(part.request) ||
      typeof part.request.conversation_id !== "string" ||
      typeof part.request.text !== "string" ||
      (part.response !== undefined &&
        (!isRecord(part.response) ||
          !isRecord(part.response.message) ||
          typeof part.response.message.id !== "string" ||
          !Number.isSafeInteger(part.response.message.sequence)))
    ) {
      throw new Error("checkpoint output is invalid");
    }
  }
  state.cursors = Object.assign(Object.create(null), state.cursors);
  state.revisions = Object.assign(Object.create(null), state.revisions);
  state.tasks = Object.assign(Object.create(null), state.tasks);
  state.parts = Object.assign(Object.create(null), state.parts);
  state.memory = Object.assign(Object.create(null), state.memory);
  for (const memory of Object.values(state.memory))
    memory.known = Object.assign(Object.create(null), memory.known);
  return state;
}
