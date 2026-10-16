// Cached worker state, persisted asynchronously as individual KV records.
// The model workspace is deliberately separate from delivery bookkeeping.
import { LEGACY_GROUP_INTERVAL_MS } from "./group.ts";
import { Records } from "./records.ts";
import { trackState } from "./tracked.ts";
import { emptyState, validate, type State } from "./state.ts";
export { STATE_VERSION, taskKey, textHash } from "./state.ts";
export type {
  TaskRecord,
  CommandAction,
  OutboxPart,
  ConversationMemory,
} from "./state.ts";
import {
  taskKey,
  type TaskRecord,
  type OutboxPart,
  type ConversationMemory,
  type Metrics,
} from "./state.ts";

export class Store {
  readonly path: string;
  readonly records: Records;
  readonly state: State;
  private readonly dirty = new Set<string>();
  private pending: Promise<void> = Promise.resolve();
  taskLimit = 200;
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

  private constructor(path: string, records: Records, state: State) {
    this.path = path;
    this.records = records;
    this.state = trackState(state, (key) => this.dirty.add(key));
  }

  static async open(path: string): Promise<Store> {
    const records = await Records.open(path);
    try {
      const state = emptyState();
      for await (const [field, id, value] of records.entries()) {
        if (field === "meta") {
          if (id === "version") state.version = value as number;
          else if (id === "account") state.account = value as string | null;
          else throw new Error("Unknown worker metadata");
        } else if (
          [
            "cursors",
            "revisions",
            "memory",
            "tasks",
            "parts",
            "sessions",
          ].includes(field)
        ) {
          (state[field as "tasks"] as Record<string, unknown>)[id] = value;
        } else throw new Error("Unknown worker record");
      }
      const store = new Store(path, records, validate(state));
      // Persist an explicit format version even for an empty database.
      store.dirty.add(JSON.stringify(["meta", "version"]));
      for (const task of store.tasks()) {
        if (
          task.replyHash === undefined &&
          task.outputHash === task.promptHash &&
          task.outputSlot &&
          task.slots.some(
            (slot) =>
              (slot === task.outputSlot ||
                slot.startsWith(`${task.outputSlot}:`)) &&
              store.part(`${taskKey(task.conversation, task.input)}:${slot}`),
          )
        )
          task.replyHash = task.promptHash;
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
      for (const task of store.tasks()) {
        if (task.autonomous && task.batch && task.status === "queued")
          task.readyAt = Math.max(
            task.readyAt ?? 0,
            (task.batchStarted ?? 0) + LEGACY_GROUP_INTERVAL_MS,
          );
      }
      for (const session of Object.values(store.state.sessions)) {
        if (session.phase === "calling") {
          session.phase = "ready";
          session.next = { kind: "continue" };
        }
        for (const job of Object.values(session.jobs)) {
          if (job.status === "running" || job.status === "queued") {
            job.status = "interrupted";
            job.result = JSON.stringify({
              error:
                "Worker interrupted; effects may already have occurred. Inspect evidence before repeating this action.",
            });
          }
        }
      }
      await store.save();
      await records.maintain();
      return store;
    } catch (error) {
      records.close();
      throw error;
    }
  }

  /** Read-only inspection never changes worker ownership or recovery state. */
  static async inspect(path: string): Promise<State> {
    for (let attempt = 0; ; attempt++) {
      const records = await Records.open(path);
      try {
        const state = emptyState();
        for await (const [field, id, value] of records.entries()) {
          if (field === "meta") Reflect.set(state, id, value);
          else (state[field as "tasks"] as Record<string, unknown>)[id] = value;
        }
        return validate(state);
      } catch (error) {
        if (attempt >= 2) throw error;
      } finally {
        records.close();
      }
    }
  }

  /** Snapshot only changed records before yielding; serialize commits in order.
   * A failed commit poisons the queue so no later effect can skip that failure. */
  save(): Promise<void> {
    this.prune(this.taskLimit);
    const changes = [...this.dirty].map((encoded) => {
      const [field, id] = JSON.parse(encoded) as [string, string];
      const value =
        field === "meta"
          ? this.state[id as "version"]
          : (this.state[field as "tasks"] as Record<string, unknown>)[id];
      return {
        field,
        id,
        json: value === undefined ? undefined : JSON.stringify(value),
      };
    });
    this.dirty.clear();
    this.pending = this.pending.then(() => this.records.commit(changes));
    return this.pending;
  }

  async close(): Promise<void> {
    try {
      await this.save();
    } finally {
      this.records.close();
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
    return this.state.memory[conversationId];
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
   * Bound durable delivery state. Terminal tasks and their recorded output are dropped
   * once more than `limit` exist; queued/running work and any unresolved output
   * are always kept, because losing those would lose a delivery guarantee.
   */
  prune(limit: number): void {
    if (!Number.isSafeInteger(limit) || limit < 1) return;
    const terminal = Object.entries(this.state.tasks)
      .filter(
        ([key, task]) =>
          !Object.entries(this.state.sessions).some(
            ([conversation, session]) =>
              conversation === task.conversation &&
              (session.root === task.input ||
                session.sources.some((source) => source.input === task.input) ||
                Object.values(session.jobs).some(
                  (job) => job.root === task.input && !job.observed,
                )) &&
              (session.phase === "calling" ||
                session.next.kind !== "wait" ||
                Object.values(session.jobs).some((job) => !job.observed)),
          ) &&
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
      for (const [key, task] of terminal.slice(0, Math.min(excess, 100))) {
        if (this.dirty.size + task.slots.length + 1 > 500) break;
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
      if (!owned.has(key) && this.dirty.size < 500)
        delete this.state.parts[key];
  }

  /** Drop state for conversations the account can no longer read. */
  forget(conversationIds: Set<string>): string[] {
    const retained = new Set(
      this.tasks()
        .filter(
          (task) =>
            task.actions?.some((action) => action.status === "pending") ||
            task.slots.some(
              (slot) =>
                this.part(`${taskKey(task.conversation, task.input)}:${slot}`)
                  ?.response === undefined,
            ),
        )
        .map((task) => taskKey(task.conversation, task.input)),
    );
    const dropped: string[] = [];
    for (const [id, session] of Object.entries(this.state.sessions)) {
      if (
        conversationIds.has(id) ||
        session.phase === "calling" ||
        Object.values(session.jobs).some(
          (job) =>
            !job.observed ||
            job.status === "queued" ||
            job.status === "running",
        ) ||
        [...retained].some((key) => this.state.tasks[key]?.conversation === id)
      )
        continue;
      delete this.state.sessions[id];
    }
    for (const [conversationId] of Object.entries(this.state.cursors)) {
      if (conversationIds.has(conversationId)) continue;
      if (this.dirty.size + 3 > 500) break;
      dropped.push(conversationId);
      delete this.state.cursors[conversationId];
      delete this.state.revisions[conversationId];
      delete this.state.memory[conversationId];
    }
    for (const [key, task] of Object.entries(this.state.tasks)) {
      if (conversationIds.has(task.conversation) || retained.has(key)) continue;
      if (this.dirty.size + task.slots.length + 1 > 500) break;
      delete this.state.tasks[key];
      for (const slot of task.slots) delete this.state.parts[`${key}:${slot}`];
      if (!dropped.includes(task.conversation)) dropped.push(task.conversation);
    }
    const owned = new Set(
      Object.entries(this.state.tasks).flatMap(([key, task]) =>
        task.slots.map((slot) => `${key}:${slot}`),
      ),
    );
    for (const [slotKey, part] of Object.entries(this.state.parts)) {
      if (
        conversationIds.has(part.request.conversation_id) ||
        owned.has(slotKey)
      )
        continue;
      if (this.dirty.size >= 500) break;
      delete this.state.parts[slotKey];
      if (!dropped.includes(part.request.conversation_id))
        dropped.push(part.request.conversation_id);
    }
    return dropped;
  }
}
