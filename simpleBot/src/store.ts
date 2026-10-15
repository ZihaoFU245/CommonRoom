// Durable worker state: input cursors, conversation revisions, the task ledger
// and the pending-output outbox.
//
// One worker owns one checkpoint file. Writes are atomic renames at mode 0600,
// which covers process interruption; use an fsynced/transactional store if you
// need power-loss durability or several workers on one account.
//
// Version 2 adds `revisions` (edit/retraction detection) and `tasks` (per-input
// work ledger). Version 1 checkpoints are refused deliberately: revising the
// dedup and revision watermarks without reconciling first can duplicate output.
import {
  closeSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { dirname } from "node:path";
import { randomUUID } from "node:crypto";

export const STATE_VERSION = 2;

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
  actions?: CommandAction[];
  slots: string[];
  /** Durable output family for the version currently being answered. */
  outputHash?: string;
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

function emptyState(): State {
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
function validate(value: unknown): State {
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
      (task.privateCommand !== undefined &&
        typeof task.privateCommand !== "boolean") ||
      (task.actions !== undefined &&
        (!Array.isArray(task.actions) ||
          task.actions.length > 8 ||
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

export class Store {
  readonly path: string;
  state: State;
  metrics: Metrics = {
    syncs: 0,
    frames: 0,
    reconnects: 0,
    sent: 0,
    replayed: 0,
    revisions: 0,
    edits: 0,
    retractions: 0,
  };

  private constructor(path: string, state: State) {
    this.path = path;
    this.state = state;
  }

  static load(path: string): Store {
    try {
      const store = new Store(
        path,
        validate(JSON.parse(readFileSync(path, "utf8"))),
      );
      for (const task of store.tasks()) {
        // Existing v2 checkpoints used answer/edit slot families without
        // recording a family explicitly. Adopt them before handling new edits.
        if (task.outputSlot === undefined && task.slots.length > 0) {
          const edited = task.slots.some(
            (slot) => slot === "edit" || slot.startsWith("edit:"),
          );
          task.outputSlot = edited ? "edit" : "answer";
          task.outputHash = task.promptHash;
          task.pass = edited ? 1 : 0;
        }
        const pending = task.slots.some((slot) => {
          const part = store.part(
            `${taskKey(task.conversation, task.input)}:${slot}`,
          );
          return part !== undefined && part.response === undefined;
        });
        if (
          task.actions?.some(
            (action) => action.status === "pending" && !action.key,
          ) ||
          (task.status === "running" &&
            task.actions?.some((action) => !action.key) &&
            task.slots.length === 0)
        ) {
          task.status = "failed";
          task.error =
            "Legacy unkeyed server command outcome uncertain; inspect before retrying.";
        } else if (
          task.actions?.some(
            (action) => action.status === "pending" && action.key,
          )
        ) {
          task.status = "queued";
        } else if (
          task.status === "running" ||
          (task.status === "failed" &&
            (pending ||
              (task.actions?.length &&
                task.actions.every((action) => action.key) &&
                task.slots.length === 0)))
        )
          task.status = "queued";
      }
      return store;
    } catch (error) {
      const code = (error as NodeJS.ErrnoException).code;
      if (code === "ENOENT") return new Store(path, emptyState());
      throw error;
    }
  }

  /** Bound applied on every save; set from `chat.stateLimit` at startup. */
  taskLimit = 200;

  /** Atomic save. Call it after each accepted message or output change. */
  save(): void {
    this.prune(this.taskLimit);
    mkdirSync(dirname(this.path), { recursive: true, mode: 0o700 });
    const temporary = `${this.path}.${randomUUID()}.tmp`;
    const handle = openSync(temporary, "w", 0o600);
    try {
      writeFileSync(handle, `${JSON.stringify(this.state)}\n`);
      fsyncSync(handle);
    } finally {
      closeSync(handle);
    }
    try {
      renameSync(temporary, this.path);
    } catch (error) {
      try {
        unlinkSync(temporary);
      } catch {
        // Leaving a stale temporary file is harmless.
      }
      throw error;
    }
  }

  cursor(conversationId: string): number | undefined {
    return this.state.cursors[conversationId];
  }

  setCursor(conversationId: string, sequence: number): void {
    this.state.cursors[conversationId] = sequence;
  }

  revision(conversationId: string): number | undefined {
    return this.state.revisions[conversationId];
  }

  setRevision(conversationId: string, revision: number): void {
    this.state.revisions[conversationId] = revision;
  }

  memory(conversationId: string): ConversationMemory {
    const existing = this.state.memory[conversationId];
    if (existing !== undefined) return existing;
    const created: ConversationMemory = {
      known: Object.create(null) as Record<string, string>,
      sequence: 0,
      latest: 0,
    };
    this.state.memory[conversationId] = created;
    return created;
  }

  /** Remember an accepted message version, trimming to the newest entries. */
  remember(
    conversationId: string,
    messageId: string,
    hash: string,
    sequence: number,
    limit = 400,
  ): void {
    const memory = this.memory(conversationId);
    memory.known[messageId] = hash;
    memory.sequence = Math.max(memory.sequence, sequence);
    memory.latest = Math.max(memory.latest, sequence);
    const ids = Object.keys(memory.known);
    if (ids.length > limit) {
      for (const id of ids.slice(0, ids.length - limit))
        delete memory.known[id];
    }
  }

  /**
   * Update the recorded version of a message we already know about. Unlike
   * `remember`, this never moves the input cursor: reconciliation is not
   * consumption.
   */
  rememberVersion(
    conversationId: string,
    messageId: string,
    hash: string,
    sequence: number,
  ): void {
    const memory = this.memory(conversationId);
    if (memory.known[messageId] === undefined) return;
    memory.known[messageId] = hash;
    memory.sequence = Math.max(memory.sequence, sequence);
    memory.latest = Math.max(memory.latest, sequence);
  }

  task(conversationId: string, messageId: string): TaskRecord | undefined {
    return this.state.tasks[taskKey(conversationId, messageId)];
  }

  putTask(task: TaskRecord): void {
    this.state.tasks[taskKey(task.conversation, task.input)] = task;
  }

  deleteTask(conversationId: string, messageId: string): void {
    delete this.state.tasks[taskKey(conversationId, messageId)];
  }

  /** All tasks, newest first, for bounded reconciliation and status output. */
  tasks(): TaskRecord[] {
    return Object.values(this.state.tasks).sort(
      (a, b) => b.updated - a.updated,
    );
  }

  part(slot: string): OutboxPart | undefined {
    return this.state.parts[slot];
  }

  putPart(part: OutboxPart): void {
    this.state.parts[part.slot] = part;
  }

  deleteParts(slots: string[]): void {
    for (const slot of slots) delete this.state.parts[slot];
  }

  /**
   * Bound the checkpoint. Terminal tasks and their recorded output are dropped
   * once more than `limit` exist; queued/running work and any unresolved output
   * are always kept, because losing those would lose a delivery guarantee.
   */
  prune(limit: number): void {
    if (!Number.isSafeInteger(limit) || limit < 1) return;
    const terminal = Object.entries(this.state.tasks)
      .filter(
        ([key, task]) =>
          task.status !== "queued" &&
          task.status !== "running" &&
          !task.actions?.some((action) => action.status === "pending") &&
          !task.slots.some(
            (slot) => this.part(`${key}:${slot}`)?.response === undefined,
          ),
      )
      .sort((a, b) => a[1].updated - b[1].updated);
    const excess = terminal.length - limit;
    if (excess > 0) {
      for (const [key, task] of terminal.slice(0, excess)) {
        delete this.state.tasks[key];
        for (const slot of task.slots)
          delete this.state.parts[`${key}:${slot}`];
      }
    }
    // Orphaned output: no task owns it any more.
    const owned = new Set(
      Object.entries(this.state.tasks).flatMap(([key, task]) =>
        task.slots.map((slot) => `${key}:${slot}`),
      ),
    );
    for (const key of Object.keys(this.state.parts))
      if (!owned.has(key)) delete this.state.parts[key];
  }

  /** Drop state for conversations the account can no longer read. */
  forget(conversationIds: Set<string>): string[] {
    if (
      this.tasks().some(
        (task) =>
          !conversationIds.has(task.conversation) &&
          task.actions?.some((action) => action.status === "pending"),
      )
    )
      throw new Error(
        "pending server command target is inaccessible; inspect checkpoint before resolving it",
      );
    for (const part of Object.values(this.state.parts)) {
      if (
        part.response === undefined &&
        !conversationIds.has(part.request.conversation_id)
      ) {
        throw new Error(
          "pending output target is inaccessible; inspect checkpoint before resolving it",
        );
      }
    }
    const dropped: string[] = [];
    for (const [conversationId] of Object.entries(this.state.cursors)) {
      if (conversationIds.has(conversationId)) continue;
      dropped.push(conversationId);
      delete this.state.cursors[conversationId];
      delete this.state.revisions[conversationId];
      delete this.state.memory[conversationId];
    }
    for (const [key, task] of Object.entries(this.state.tasks)) {
      if (conversationIds.has(task.conversation)) continue;
      delete this.state.tasks[key];
    }
    for (const [slot, part] of Object.entries(this.state.parts)) {
      if (conversationIds.has(part.request.conversation_id)) continue;
      delete this.state.parts[slot];
    }
    return dropped;
  }
}
