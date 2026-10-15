import { Execution } from "./execution.ts";
import { systemPrompt } from "./prompts.ts";
import { modelTools } from "./tools.ts";
import { Memory } from "./memory.ts";
import {
  GROUP_INTERVAL_MS,
  MAX_GROUP_RESPONSE_CHARS,
  groupInputs,
  groupResponses,
} from "./group.ts";
import { abortable } from "./async.ts";
import { setTimeout as wait } from "node:timers/promises";
// The agent: snapshot catch-up, revision reconciliation, command handling and
// durable output.
//
// Ordering rules that keep this correct:
//  1. A message is handled only after its input is durably recorded, and its
//     cursor advances only after every output for it is acknowledged.
//  2. Output requests are saved before they are sent, so a crash replays the
//     same idempotency key instead of producing a second message.
//  3. Revision rescans are pinned to one `content_revision`; a mid-scan change
//     returns `conflict` and the scan restarts rather than diffing a torn view.
import { BotApi, BotApiError, chunkMessage, idempotencyKey } from "./api.ts";
import {
  isConfigKey,
  quotedValue,
  formatConfig,
  type ConfigStore,
} from "./config.ts";
import type { Logger } from "./log.ts";
import type { ChatMessage, ChatModel } from "./model.ts";
import { AppendHistory, ContextWindow } from "./context.ts";
import { Metrics } from "./metrics.ts";
import { compactConversation } from "./tokens.ts";
import {
  Store,
  taskKey,
  textHash,
  type CommandAction,
  type TaskRecord,
} from "./store.ts";
import type {
  BotCommand,
  Conversation,
  HistoryResponse,
  Message,
  Snapshot,
} from "./wire.ts";

export interface AgentOptions {
  api: BotApi;
  store: Store;
  config: ConfigStore;
  model: ChatModel;
  /** Persistent shared filesystem workspace, separate from operator data. */
  workspace: string;
  log: Logger;
  /** Presence is only delivered to account-wide credentials. */
  presence(): string[];
  socketReady(): boolean;
  setActivity(conversationId: string, state: "working" | "idle"): void;
}

const PUBLIC_COMMANDS: BotCommand[] = [
  {
    name: "/metrics",
    usage: "/metrics",
    description: "Model usage and cache metrics for this conversation",
  },
  { name: "/help", usage: "/help", description: "How to chat with this bot" },
  {
    name: "/ping",
    usage: "/ping",
    description: "Check that the worker is available",
  },
];
const WORKER_COMMANDS: BotCommand[] = [
  {
    name: "/reload",
    usage: "/reload",
    description: "Reload validated settings from the worker configuration file",
    operator_only: true,
  },
  {
    name: "/admin",
    usage: "/admin",
    description: "List this bot's owner/admin controls",
    operator_only: true,
  },
  {
    name: "/whoami",
    usage: "/whoami",
    description: "Show this bot's identity and owner",
    operator_only: true,
  },
  {
    name: "/owner",
    usage: "/owner",
    description: "Show who owns this bot",
    operator_only: true,
  },
  {
    name: "/status",
    usage: "/status",
    description: "Inspect this worker's status",
    operator_only: true,
  },
  {
    name: "/features",
    usage: "/features",
    description: "Inspect this worker's implementation features",
    operator_only: true,
  },
  {
    name: "/model",
    usage: "/model",
    description: "Inspect model configuration",
    operator_only: true,
  },
  {
    name: "/config",
    usage: '/config get [key] | set key "value"',
    description: "Read or change this bot's configuration",
    operator_only: true,
    subcommands: [
      {
        name: "get",
        usage: "/config get [key]",
        description: "Read configuration with secrets redacted",
      },
      {
        name: "set",
        usage: '/config set key "value"',
        description: "Change a model or worker setting",
      },
    ],
  },
];

const GROUP_CHANGED = new Error("Group input changed");

const ANSWER_SLOT = "answer";
const EDIT_SLOT = "edit";

export class Agent {
  async commands(conversationId: string): Promise<BotCommand[]> {
    const credential = this.snapshot?.credential;
    if (!this.snapshot?.conversations.some((c) => c.id === conversationId))
      return [];
    const privileged =
      credential?.operations === null ||
      credential?.operations.includes("config");
    return [...PUBLIC_COMMANDS, ...(privileged ? WORKER_COMMANDS : [])];
  }

  private async serverCommands(conversationId: string): Promise<BotCommand[]> {
    if (!this.snapshot?.conversations.some((c) => c.id === conversationId))
      return [];
    try {
      // Replies are published once through the durable outbox. Exposing these
      // send commands would let a model publish a second answer as a tool effect.
      return (
        await this.options.api.serverCommands(conversationId)
      ).commands.filter(
        (command) => !["/reply", "/tell"].includes(command.name),
      );
    } catch (error) {
      if (error instanceof BotApiError && [403, 404].includes(error.status))
        return [];
      throw error;
    }
  }
  /** Read-only diagnostics for the worker's owner/admin chat commands. */
  operatorValues(): Record<string, string> {
    const conversation = this.snapshot?.conversations[0];
    const config = this.options.config;
    return {
      "worker.help":
        "Admin worker diagnostics: use @BOT /status, /features or /model. Chat requests use the bot account's server commands and grants.",
      "worker.status": conversation
        ? this.status(conversation)
        : "No readable conversation is available.",
      "worker.features": conversation
        ? this.features(conversation)
        : "No readable conversation is available.",
      "worker.model": `Model: ${this.options.model.describe}\nStreaming: ${config.get("model.stream")}\nOutput token limit: ${config.get("model.maxTokens")}\nReasoning effort: ${config.get("model.reasoningEffort") || "provider default"}\nTimeout: ${config.get("model.timeoutMs")} ms`,
    };
  }
  private readonly options: AgentOptions;
  private snapshot: Snapshot | null = null;
  private syncing = false;
  private dirty = false;
  private drainRequested = false;
  private readonly groupWindows = new Map<string, number>();
  private draining = false;
  private activityTimer: NodeJS.Timeout | null = null;
  /** Conversation currently marked working, so idle goes to the same one. */
  private activityConversation: string | null = null;
  private startedAt = Date.now();
  private stopped = false;
  private readonly abort = new AbortController();
  private activeTask: { key: string; controller: AbortController } | undefined;
  private readonly execution: Execution;
  private readonly contexts = new Map<string, AppendHistory>();
  private readonly modelMetrics = new Map<string, Metrics>();

  private metrics(conversation: string): Metrics {
    let value = this.modelMetrics.get(conversation);
    if (!value) value = new Metrics();
    this.modelMetrics.delete(conversation);
    this.modelMetrics.set(conversation, value);
    if (this.modelMetrics.size > 64)
      this.modelMetrics.delete(this.modelMetrics.keys().next().value!);
    return value;
  }

  constructor(options: AgentOptions) {
    this.options = options;
    this.execution = new Execution(options.workspace);
  }

  get directory(): Snapshot | null {
    return this.snapshot;
  }

  get busy(): boolean {
    return this.syncing || this.draining;
  }

  setModel(model: ChatModel): void {
    this.options.model = model;
  }

  /** Quiesce before replacing the transport so two agents cannot share the outbox. */
  async stop(): Promise<void> {
    this.stopped = true;
    this.abort.abort();
    this.endActivity();
    while (this.busy) await new Promise((resolve) => setTimeout(resolve, 10));
    await this.execution.settle();
  }

  // ---------------------------------------------------------------- catch-up

  /** Apply a snapshot and converge on the server's retained state. */
  async handleSnapshot(snapshot: Snapshot): Promise<void> {
    if (this.stopped) return;
    const store = this.options.store;
    if (
      store.state.account !== null &&
      store.state.account !== snapshot.account.id
    ) {
      throw new Error(
        `checkpoint belongs to account ${store.state.account}, but this credential is ${snapshot.account.id}`,
      );
    }
    this.snapshot = snapshot;
    // Await the catch-up so callers (and tests) can observe a settled state.
    await this.requestSync();
  }

  /**
   * Converge without processing new input. This is what a socket signal causes:
   * the notification's revision is a hint, and the fresh snapshot decides.
   */
  async sync(): Promise<void> {
    await this.requestSync(false);
  }

  /**
   * Coalesce signals: at most one sync runs, with one rerun queued. Draining
   * (which may call the model) is skipped when `drainWork` is false.
   */
  async requestSync(drainWork = true): Promise<void> {
    if (this.stopped) return;
    this.drainRequested ||= drainWork;
    if (this.syncing) {
      this.dirty = true;
      return;
    }
    this.syncing = true;
    try {
      do {
        this.dirty = false;
        await this.syncOnce();
      } while (this.dirty && !this.stopped);
    } finally {
      this.syncing = false;
    }
    if (this.drainRequested && !this.stopped) {
      this.drainRequested = false;
      await this.drain();
    }
  }

  private async syncOnce(): Promise<void> {
    const { api, store, log } = {
      api: this.options.api,
      store: this.options.store,
      log: this.options.log,
    };
    const snapshot = await api.me();
    let save = store.state.account === null;
    if (
      store.state.account !== null &&
      store.state.account !== snapshot.account.id
    ) {
      throw new Error("checkpoint belongs to another account");
    }
    store.state.account = snapshot.account.id;
    this.snapshot = snapshot;
    store.metrics.syncs++;

    const live = new Set(
      snapshot.conversations.map((conversation) => conversation.id),
    );
    for (const id of this.contexts.keys())
      if (!live.has(id)) this.contexts.delete(id);
    for (const id of this.modelMetrics.keys())
      if (!live.has(id)) this.modelMetrics.delete(id);
    // Resolve known outcomes before discarding inaccessible conversations or
    // locating retracted inputs. A status read never submits a new effect.
    for (const task of store.tasks()) {
      if (task.status === "running") continue;
      const pending =
        task.actions?.filter(
          (action) => action.status === "pending" && action.key,
        ) ?? [];
      let recovered = true;
      for (const action of pending) {
        try {
          await abortable(
            this.recoverCommand(task.conversation, action, false),
            AbortSignal.timeout(3000),
          );
        } catch (error) {
          recovered = false;
          task.error =
            error instanceof Error ? error.message : "Command recovery failed";
          log("warn", `recovery deferred for ${task.input}: ${task.error}`);
        }
      }
      if (
        recovered &&
        pending.length &&
        task.actions?.every((action) => action.status === "done") &&
        task.status === "failed"
      ) {
        task.status = "queued";
        delete task.error;
        save = true;
      }
    }
    const dropped = store.forget(live);
    save ||= dropped.length > 0;
    if (dropped.length > 0)
      log(
        "info",
        `dropped state for ${dropped.length} inaccessible conversation(s)`,
      );

    for (const conversation of snapshot.conversations) {
      if (conversation.unread === null) {
        // Metadata is temporarily unavailable: defer rather than baseline at 0.
        log(
          "debug",
          `deferring ${conversation.view}: unread metadata unavailable`,
        );
        continue;
      }
      const cursor = store.cursor(conversation.id);
      if (cursor === undefined) {
        store.setCursor(conversation.id, conversation.unread.through);
        store.setRevision(conversation.id, conversation.content_revision);
        await store.save();
        log(
          "info",
          `baseline ${conversation.view} at sequence ${conversation.unread.through}`,
        );
        continue;
      }
      if (cursor > 0 && cursor < conversation.unread.oldest) {
        log(
          "warn",
          `retention gap in ${conversation.view}: cursor ${cursor} predates oldest retained ${conversation.unread.oldest}`,
        );
      }
      const previous = store.revision(conversation.id);
      if (
        previous === conversation.content_revision &&
        cursor >= conversation.unread.through
      )
        continue;
      if (
        previous !== undefined &&
        previous !== conversation.content_revision
      ) {
        await this.reconcile(conversation, previous);
      } else if (previous === undefined) {
        store.setRevision(conversation.id, conversation.content_revision);
        save = true;
      }
      await this.advance(conversation);
    }
    if (save) await store.save();
  }

  /**
   * Read new input after the saved cursor.
   *
   * The cursor advances only for messages we consumed or that need no answer
   * (our own output, or chatter that does not address this bot). Messages that
   * are queued stay *above* the cursor until their output is acknowledged, so a
   * crash mid-answer replays them instead of skipping them.
   */
  private async advance(conversation: Conversation): Promise<void> {
    const { store } = this.options;
    const accountId = this.snapshot?.account.id;
    let after = store.cursor(conversation.id) ?? 0;
    let blocked = false;
    for (;;) {
      const page = await this.history({
        conversation_id: conversation.id,
        after,
        limit: 100,
      });
      this.options.log(
        "debug",
        `input history page conversation=${conversation.id} after=${after} messages=${page.messages.length} has_after=${page.has_after}`,
      );
      if (page.messages.length === 0) return;
      for (const message of page.messages) {
        if (message.sequence <= (store.cursor(conversation.id) ?? 0)) continue;
        store.remember(
          conversation.id,
          message.id,
          textHash(message.text),
          message.sequence,
        );
        // Our own messages are recorded so they are never re-scanned, but they
        // never become work and never block the cursor.
        if (message.author_id !== accountId)
          this.consider(conversation, message);
        else
          this.options.log(
            "debug",
            `input skipped conversation=${conversation.id} input=${message.id} sequence=${message.sequence} reason=own_message`,
          );
        const parkedTask = store.task(conversation.id, message.id);
        const parked =
          parkedTask !== undefined &&
          !["answered", "cancelled"].includes(parkedTask.status);
        blocked ||= parked;
        if (!blocked) store.setCursor(conversation.id, message.sequence);
        after = Math.max(after, message.sequence);
        // Durable before the cursor moves: a crash replays rather than skips.
        await store.save();
      }
      if (page.has_after) continue;
      return;
    }
  }

  // ----------------------------------------------------- revision reconcile

  /**
   * `content_revision` moved without new sequence numbers: an edit, retraction,
   * reaction or cleanup happened somewhere in retained history. Rescan one
   * pinned revision and diff against what we accepted.
   *
   * `conversation.content_revision` is the revision this scan is pinned to, and
   * `previous` is the revision we last accepted.
   */
  private async reconcile(
    conversation: Conversation,
    previous: number,
  ): Promise<void> {
    const { store, log } = this.options;
    const revision = conversation.content_revision;
    log(
      "info",
      `revision ${previous} -> ${revision} in ${conversation.view}: rescanning retained history`,
    );
    const seen = new Map<string, string>();
    let around = 0;
    for (;;) {
      let scanned: HistoryResponse;
      try {
        scanned = await this.history({
          conversation_id: conversation.id,
          after: around,
          limit: 100,
          content_revision: revision,
        });
      } catch (error) {
        if (error instanceof BotApiError && error.code === "conflict") {
          // History changed mid-scan; the next sync handles the newer revision.
          log(
            "debug",
            `rescan of ${conversation.view} lost a race; will retry on the next signal`,
          );
          this.dirty = true;
          return;
        }
        throw error;
      }
      for (const message of scanned.messages) {
        seen.set(message.id, textHash(message.text));
        around = Math.max(around, message.sequence);
      }
      if (!scanned.has_after || scanned.messages.length === 0) break;
    }
    store.metrics.revisions++;
    const memory = store.memory(conversation.id);
    for (const [id, hash] of seen) {
      const accepted = memory.known[id];
      if (accepted === undefined || accepted === hash) continue;
      store.metrics.edits++;
      log("info", `detected an edit of message ${id} in ${conversation.view}`);
      this.replaceTask(conversation, id, hash);
    }
    for (const id of Object.keys(memory.known)) {
      if (seen.has(id)) continue;
      store.metrics.retractions++;
      log(
        "info",
        `message ${id} is no longer retained in ${conversation.view}`,
      );
      this.cancelTask(conversation, id);
      delete memory.known[id];
    }
    // Record the scanned versions without touching the input cursor.
    for (const [id, hash] of seen)
      store.rememberVersion(conversation.id, id, hash, 0);
    store.setRevision(conversation.id, revision);
    await store.save();
  }

  /**
   * An input we already answered changed. Update the recorded version in place
   * and let the drain loop pick it up again; delivered parts stay in the
   * transcript so the revision pass is additive rather than a correction.
   */
  private replaceTask(
    conversation: Conversation,
    messageId: string,
    hash: string,
  ): void {
    const store = this.options.store;
    const task = store.task(conversation.id, messageId);
    if (task === undefined) {
      // An edit of a message this worker never processed: nothing to revise.
      return;
    }
    const chat = this.snapshot?.conversations.find(
      (candidate) => candidate.id === conversation.id,
    );
    this.changedBatch(conversation, task);
    if (task.autonomous && task.status === "answered") {
      delete task.batch;
      delete task.decision;
      task.readyAt = Date.now() + GROUP_INTERVAL_MS;
    }
    task.promptHash = hash;
    task.replyTo = chat?.permissions.reply ? messageId : null;
    task.status = "queued";
    task.updated = Date.now();
    store.putTask(task);
  }

  /** A pending input vanished: stop its unsent output, keep what was delivered. */
  private cancelTask(conversation: Conversation, messageId: string): void {
    const store = this.options.store;
    const task = store.task(conversation.id, messageId);
    if (task === undefined) return;
    this.changedBatch(conversation, task);
    const key = taskKey(conversation.id, messageId);
    const unsent = task.slots.filter(
      (slot) => store.part(`${key}:${slot}`)?.response === undefined,
    );
    store.deleteParts(unsent.map((slot) => `${key}:${slot}`));
    task.slots = task.slots.filter((slot) => !unsent.includes(slot));
    task.status = "cancelled";
    task.updated = Date.now();
    if (this.activeTask?.key === key) {
      this.options.log(
        "debug",
        `task model cancelled conversation=${conversation.id} input=${messageId} reason=input_removed`,
      );
      this.activeTask.controller.abort();
    }
  }

  // ------------------------------------------------------------ work queue

  private controlName(
    conversation: Conversation,
    message: Message,
  ): string | undefined {
    const account = this.snapshot?.account;
    if (
      !account ||
      !(
        message.mentions.includes(account.username) ||
        conversation.view.startsWith("@direct:")
      )
    )
      return undefined;
    const prefix = `@${account.username}`;
    const body = message.text.startsWith(prefix)
      ? message.text.slice(prefix.length).trim()
      : message.text.trim();
    const name = body.split(/\s/u)[0]?.toLowerCase();
    return [...PUBLIC_COMMANDS, ...WORKER_COMMANDS].some(
      (command) => command.name === name,
    )
      ? name
      : undefined;
  }

  private changedBatch(conversation: Conversation, task: TaskRecord): void {
    const leader = task.batchOwner
      ? this.options.store.task(conversation.id, task.batchOwner)
      : task.batch
        ? task
        : undefined;
    if (
      leader &&
      this.activeTask?.key === taskKey(conversation.id, leader.input)
    )
      this.activeTask.controller.abort(GROUP_CHANGED);
  }

  /** Decide whether an input deserves an answer, then record that decision. */
  private consider(conversation: Conversation, message: Message): void {
    const account = this.snapshot?.account;
    if (account === undefined || message.author_id === account.id) return;
    const autonomous =
      this.options.config.get("autonomous") &&
      !conversation.view.startsWith("@");
    const addressed =
      autonomous ||
      message.mentions.includes(account.username) ||
      (conversation.view.startsWith("@direct:") &&
        this.options.config.get("chat.respondToAllDms"));
    const existing = this.options.store.task(conversation.id, message.id);
    if (!addressed || !conversation.permissions.send || existing) {
      this.options.log(
        "debug",
        `input skipped conversation=${conversation.id} input=${message.id} sequence=${message.sequence} reason=${!addressed ? "not_addressed" : !conversation.permissions.send ? "send_not_permitted" : "existing_task"}${existing ? ` status=${existing.status}` : ""}`,
      );
      return;
    }
    const grouped = autonomous && !this.controlName(conversation, message);
    if (grouped && !this.groupWindows.has(conversation.id))
      this.groupWindows.set(conversation.id, Date.now() + GROUP_INTERVAL_MS);
    this.options.store.putTask({
      ...(grouped
        ? {
            autonomous: true,
            readyAt:
              this.groupWindows.get(conversation.id) ??
              Date.now() + GROUP_INTERVAL_MS,
          }
        : {}),
      conversation: conversation.id,
      input: message.id,
      sequence: message.sequence,
      promptHash: textHash(message.text),
      inputChars: message.text.length,
      replyTo: conversation.permissions.reply ? message.id : null,
      status: "queued",
      attempts: 0,
      slots: [],
      updated: Date.now(),
    });
    this.options.log(
      "debug",
      `task queued conversation=${conversation.id} input=${message.id} sequence=${message.sequence}`,
    );
  }
  private async drain(): Promise<void> {
    if (this.draining || this.stopped) return;
    this.draining = true;
    try {
      while (!this.stopped) {
        if (!this.options.config.get("autonomous")) {
          this.groupWindows.clear();
          let changed = false;
          for (const candidate of this.options.store.tasks()) {
            if (
              candidate.status === "queued" &&
              candidate.autonomous &&
              !candidate.batch &&
              !candidate.batchOwner &&
              !candidate.actions?.length &&
              !candidate.slots.length
            ) {
              candidate.status = "cancelled";
              changed = true;
            }
          }
          if (changed) await this.options.store.save();
        }
        const queued = this.options.store
          .tasks()
          .filter(
            (candidate) =>
              candidate.status === "queued" && !candidate.batchOwner,
          )
          .sort(
            (a, b) =>
              Number(!!a.autonomous) - Number(!!b.autonomous) ||
              a.sequence - b.sequence,
          );
        if (!queued.length) break;
        const task = queued.find(
          (candidate) =>
            !candidate.autonomous || (candidate.readyAt ?? 0) <= Date.now(),
        );
        if (!task) {
          const deadline = Math.min(
            ...queued.map((candidate) => candidate.readyAt ?? 0),
          );
          await wait(Math.max(1, deadline - Date.now()), undefined, {
            signal: this.abort.signal,
          });
          if (this.stopped) break;
          await this.requestSync(false);
          continue;
        }
        const conversation = this.snapshot?.conversations.find(
          (candidate) => candidate.id === task.conversation,
        );
        if (!conversation) {
          task.status = "cancelled";
          await this.options.store.save();
          continue;
        }
        if (task.autonomous) {
          await this.runGroup(conversation, task);
          continue;
        }
        let acceptedHash = task.promptHash;
        const active = {
          key: taskKey(task.conversation, task.input),
          controller: new AbortController(),
        };
        this.activeTask = active;
        task.status = "running";
        task.attempts++;
        task.updated = Date.now();
        const started = performance.now();
        this.options.log(
          "debug",
          `task started conversation=${conversation.id} input=${task.input} attempt=${task.attempts}`,
        );
        await this.options.store.save();
        this.beginActivity(conversation.id);
        const cancelled = () => task.status === "cancelled";
        try {
          const message = await this.locate(conversation, task.input);
          if (!message) {
            this.cancelTask(conversation, task.input);
            continue;
          }
          // Locate can race an edit/rescan. Accept the version actually being
          // answered so a later signal cannot queue the same version again.
          acceptedHash = textHash(message.text);
          task.promptHash = acceptedHash;
          this.options.store.rememberVersion(
            conversation.id,
            message.id,
            acceptedHash,
            message.sequence,
          );
          await this.answer(conversation, message, task);
          if (!cancelled()) {
            task.status =
              task.promptHash === acceptedHash ? "answered" : "queued";
            if (task.status === "answered")
              this.options.store.setCursor(
                task.conversation,
                Math.max(
                  this.options.store.cursor(task.conversation) ?? 0,
                  task.sequence,
                ),
              );
          }
        } catch (error) {
          if (!cancelled()) {
            task.status =
              this.stopped && !task.actions?.length ? "queued" : "failed";
            task.error = error instanceof Error ? error.message : "Task failed";
            if (!this.stopped)
              this.options.log(
                "error",
                `task ${task.input} failed: ${task.error}`,
              );
          }
        } finally {
          if (this.activeTask === active) this.activeTask = undefined;
          this.options.log(
            "debug",
            `task finished conversation=${conversation.id} input=${task.input} status=${task.status} duration_ms=${Math.round(performance.now() - started)}`,
          );
          task.updated = Date.now();
          if (
            !this.options.store
              .tasks()
              .some((candidate) => candidate.status === "queued")
          )
            this.endActivity();
          await this.options.store.save();
        }
      }
    } finally {
      this.endActivity();
      this.draining = false;
    }
  }
  private async runGroup(
    conversation: Conversation,
    task: TaskRecord,
  ): Promise<void> {
    const { store, config } = this.options;
    if (!task.batch) {
      const members = store
        .tasks()
        .filter(
          (candidate) =>
            candidate.conversation === conversation.id &&
            candidate.autonomous &&
            candidate.status === "queued" &&
            !candidate.batchOwner &&
            !candidate.batch,
        )
        .sort((a, b) => a.sequence - b.sequence)
        .slice(0, 100);
      task.batch = groupInputs(members, task);
      delete task.decision;
      task.pass = (task.pass ?? -1) + 1;
      task.outputSlot = `group:${task.pass}`;
    }
    const batch = task.batch;
    task.batchStarted = Date.now();
    this.groupWindows.delete(conversation.id);
    const active = {
      key: taskKey(conversation.id, task.input),
      controller: new AbortController(),
    };
    this.activeTask = active;
    const signal = AbortSignal.any([
      active.controller.signal,
      this.abort.signal,
      AbortSignal.timeout(config.get("model.timeoutMs")),
    ]);
    for (const input of batch) {
      const member = store.task(conversation.id, input.input);
      if (member && member.status !== "cancelled") {
        member.status = "running";
        if (member !== task) member.batchOwner = task.input;
      }
    }
    task.attempts++;
    try {
      await store.save();
      this.options.log(
        "debug",
        `group turn started conversation=${conversation.id} inputs=${batch.length}`,
      );
      await abortable(this.answerGroup(conversation, task, signal), signal);
      for (const input of batch) {
        const member = store.task(conversation.id, input.input);
        if (member && member.status !== "cancelled") {
          member.status =
            member.promptHash === input.hash ? "answered" : "queued";
          delete member.batchOwner;
        }
      }
      // Do not skip an earlier queued/failed task when advancing the watermark.
      const through = Math.max(...batch.map((input) => input.sequence));
      const blocked = store
        .tasks()
        .filter(
          (member) =>
            member.conversation === conversation.id &&
            member.sequence <= through &&
            ["queued", "running"].includes(member.status),
        );
      if (!blocked.length)
        store.setCursor(
          conversation.id,
          Math.max(store.cursor(conversation.id) ?? 0, through),
        );
      if (task.status === "queued") {
        delete task.batch;
        delete task.decision;
      }
    } catch (error) {
      const changed =
        error === GROUP_CHANGED ||
        active.controller.signal.reason === GROUP_CHANGED ||
        (error instanceof BotApiError && error.code === "conflict");
      for (const input of batch) {
        const member = store.task(conversation.id, input.input);
        if (!member || member.status === "cancelled") continue;
        member.status = changed || this.stopped ? "queued" : "failed";
        member.error =
          error instanceof Error ? error.message : "Group turn failed";
        if (changed) delete member.batchOwner;
      }
      if (changed) {
        delete task.batch;
        delete task.decision;
      }
      if (!this.stopped)
        this.options.log(
          "warn",
          `group turn ${task.input} ended: ${error instanceof Error ? error.message : "failed"}`,
        );
    } finally {
      // First burst waits a window, and every following turn starts >=3s later.
      const next = Math.max(
        Date.now(),
        (task.batchStarted ?? 0) + GROUP_INTERVAL_MS,
      );
      for (const member of store.tasks())
        if (
          member.conversation === conversation.id &&
          member.autonomous &&
          member.status === "queued"
        )
          member.readyAt = Math.max(member.readyAt ?? 0, next);
      if (this.activeTask === active) this.activeTask = undefined;
      this.endActivity();
      await store.save();
    }
  }

  private async answerGroup(
    conversation: Conversation,
    task: TaskRecord,
    signal: AbortSignal,
  ): Promise<void> {
    const { store } = this.options;
    if (!task.decision) {
      const inputs = task.batch ?? [];
      const wanted = new Set(inputs.map((input) => input.input));
      const messages: Message[] = [];
      let start = Math.min(...inputs.map((input) => input.sequence));
      const end = Math.max(...inputs.map((input) => input.sequence));
      let revision: number | undefined;
      for (;;) {
        const page = await this.history({
          conversation_id: conversation.id,
          start,
          end,
          limit: 100,
          ...(revision === undefined ? {} : { content_revision: revision }),
        });
        signal.throwIfAborted();
        revision = page.content_revision;
        messages.push(
          ...page.messages.filter((message) => wanted.has(message.id)),
        );
        const last = page.messages.at(-1)?.sequence;
        if (!page.has_after || last === undefined || last >= end) break;
        start = last + 1;
      }
      let changed = false;
      for (const input of inputs) {
        const current = messages.find((message) => message.id === input.input);
        if (!current) {
          this.cancelTask(conversation, input.input);
          changed = true;
        } else if (textHash(current.text) !== input.hash) {
          this.replaceTask(conversation, input.input, textHash(current.text));
          changed = true;
        }
      }
      if (changed) throw GROUP_CHANGED;
      const first = messages[0];
      if (!first) return;
      const text = await this.askModel(
        conversation,
        first,
        "new",
        task,
        messages,
        signal,
      );
      signal.throwIfAborted();
      if (text.length > MAX_GROUP_RESPONSE_CHARS)
        throw new Error("Group response exceeds the worker output limit");
      task.decision ??= text.trim() ? [{ text }] : [];
      await store.save();
    }
    if (task.decision.length) this.beginActivity(conversation.id);
    const slot = task.outputSlot ?? "group:0";
    for (const [index, response] of task.decision.entries()) {
      this.stageParts(
        conversation,
        task,
        `${slot}:${index}`,
        response.text,
        response.reply_to,
      );
    }
    await store.save();
    signal.throwIfAborted();
    await this.deliverParts(conversation, task, slot, signal);
  }

  private async locate(
    conversation: Conversation,
    messageId: string,
  ): Promise<Message | undefined> {
    const sequence = this.options.store.task(
      conversation.id,
      messageId,
    )?.sequence;
    if (sequence === undefined) return undefined;
    const page = await this.history({
      conversation_id: conversation.id,
      around: sequence,
      limit: 1,
    });
    return page.messages.find((message) => message.id === messageId);
  }
  private async answer(
    conversation: Conversation,
    message: Message,
    task: TaskRecord,
  ): Promise<void> {
    if (task.silentHash === task.promptHash) return;
    if (task.outputSlot !== undefined && task.outputHash !== task.promptHash)
      await this.deliverParts(conversation, task, task.outputSlot);
    if (task.outputSlot === undefined) {
      task.outputSlot = ANSWER_SLOT;
      task.outputHash = task.promptHash;
      task.pass = 0;
    } else if (task.outputHash !== task.promptHash) {
      task.pass = (task.pass ?? 0) + 1;
      task.outputSlot = `${EDIT_SLOT}:${task.pass}`;
      task.outputHash = task.promptHash;
    }
    await this.options.store.save();
    const slot = task.outputSlot;
    if (await this.deliverParts(conversation, task, slot)) return;
    const prefix = `@${this.snapshot?.account.username ?? "bot"}`;
    const body =
      message.text.startsWith(prefix) &&
      /^\s/u.test(message.text.slice(prefix.length))
        ? message.text.slice(prefix.length).trim()
        : message.text.trim();
    const name = (body.split(/\s/u)[0] ?? "").toLowerCase();
    if (PUBLIC_COMMANDS.some((command) => command.name === name)) {
      const response =
        name === "/ping"
          ? "Pong."
          : name === "/metrics"
            ? this.metrics(conversation.id).report() +
              `\nLimits: compact at ${this.options.config.get("model.compactTokens")} tokens; total ${this.options.config.get("model.contextTokens")} tokens (local estimates, including output reserve).`
            : "Mention me or send a direct message to ask a question. Reply to an older message to include its context; I can fetch more history when needed. Public commands: /help, /ping, /metrics.";
      await this.sendParts(conversation, task, slot, response);
      return;
    }
    if (WORKER_COMMANDS.some((command) => command.name === name)) {
      task.privateCommand = true;
      await this.options.store.save();
      let response: string;
      try {
        const authorization = await this.options.api.messageContext(
          conversation.id,
          message.id,
        );
        if (
          authorization.bot.id !== this.snapshot?.account.id ||
          authorization.author.id !== message.author_id
        )
          throw new Error(
            "Source identity did not match the authenticated message.",
          );
        if (
          !authorization.author.groups.includes("su") &&
          (authorization.bot.groups.includes("su") ||
            (!authorization.author.groups.includes("admin") &&
              authorization.author.id !== authorization.bot.owner_id))
        )
          response =
            "Only this bot's owner or a server administrator can use its private commands.";
        else if (name === "/whoami")
          response = `I am ${this.snapshot?.account.username} (${this.snapshot?.account.id}). Owner: ${authorization.bot.owner ?? "server administrators"}.`;
        else if (name === "/owner")
          response = `Owner: ${authorization.bot.owner ?? "server administrators"}.`;
        else if (name === "/admin")
          response = WORKER_COMMANDS.map(
            (command) =>
              `- \`@${this.snapshot?.account.username} ${command.usage}\` — ${command.description}`,
          ).join("\n");
        else if (name === "/config") {
          const argument = body.slice(name.length).trim();
          const update = /^set\s+(\S+)(?:\s+([\s\S]*))?$/u.exec(argument);
          if (update) {
            const key = update[1];
            if (!key || !isConfigKey(key))
              throw new Error(`Unknown configuration key ${key}`);
            response = formatConfig(
              this.options.config.apply(key, quotedValue(update[2] ?? "")),
            );
          } else if (/^get(?:\s|$)/u.test(argument))
            response = formatConfig(
              this.options.config.fetch(
                argument.slice(3).trim() === "."
                  ? null
                  : argument.slice(3).trim() || null,
              ),
            );
          else response = 'Usage: /config get [key] | /config set key "value"';
        } else if (name === "/reload") {
          this.options.config.reload();
          response =
            "Configuration reloaded.\n" +
            formatConfig(this.options.config.fetch(null));
        } else
          response =
            this.operatorValues()[`worker.${name.slice(1)}`] ??
            "No report is available.";
      } catch (error) {
        response =
          error instanceof Error ? error.message : "Private command failed";
      }
      await this.sendParts(conversation, task, slot, response);
      return;
    }
    const text = await this.askModel(
      conversation,
      message,
      (task.pass ?? 0) > 0 ? "revised" : "new",
      task,
    );
    if (task.status === "cancelled") return;
    if (!text.trim()) {
      task.silentHash = textHash(message.text);
      await this.options.store.save();
      return;
    }
    await this.sendParts(conversation, task, slot, text);
  }

  private features(conversation: Conversation): string {
    const { store, config } = this.options;
    const credential = this.snapshot?.credential;
    const operations =
      credential?.operations?.join(", ") ?? "all (account-wide credential)";
    return [
      "**Live in this worker**",
      "- Keyed idempotent sends with a durable outbox and stable per-output slots",
      "- Multi-part output: text over 4,000 characters is split and numbered",
      "- Replies (`reply_to`) when the account holds `/reply`, plus reactions",
      "- Revision reconciliation: edits and retractions trigger a pinned rescan",
      "- Transient `working`/`idle` activity signals",
      "- Optional autonomous group turns with fixed 3-second batching and mixed reply plans",
      "- Persistent KV memory and a shared read/write data/workspace filesystem",
      "- Network-enabled JS/TS execution in restricted, bounded Deno subprocesses",
      "- Owner/admin configuration through @Bot /config, with redacted values",
      "- Presence directory (account-wide credentials only)",
      `- Operations on this credential: ${operations}`,
      "",
      "**Around it**",
      "- `POST /credentials` issues scoped, expiring worker tokens (operator action)",
      "- `POST /conversations` creates rooms or direct pairs when granted",
      "- Read acknowledgement through an explicitly enabled model tool",
      "- History cursor modes: after, before, around, start/end, revision-pinned",
      "",
      `Conversation \`${conversation.view}\`: revision ${conversation.content_revision}, ` +
        `unread.through ${conversation.unread?.through ?? "unknown"}, ` +
        `autonomous=${config.get("autonomous")}, respondToAllDms=${config.get("chat.respondToAllDms")}.`,
      `This session: ${store.metrics.syncs} syncs, ${store.metrics.edits} edits, ${store.metrics.retractions} retractions, ${store.metrics.sent} messages sent (${store.metrics.replayed} replays).`,
    ].join("\n");
  }

  private status(conversation: Conversation): string {
    const { store, config } = this.options;
    const credential = this.snapshot?.credential;
    const pending = store
      .tasks()
      .filter(
        (task) => task.status === "queued" || task.status === "running",
      ).length;
    const conversations = this.snapshot?.conversations ?? [];
    const cursor = store.cursor(conversation.id) ?? 0;
    const uptime = Math.round((Date.now() - this.startedAt) / 1000);
    return [
      `Account: ${this.snapshot?.account.username} (${this.snapshot?.account.id})`,
      `Credential: ${credential?.id ?? "unknown"}${credential?.expires ? ` expires ${new Date(credential.expires * 1000).toISOString()}` : " (no expiry)"}`,
      `Conversation scopes: ${credential?.conversations === null || credential?.conversations === undefined ? "account-wide" : `${credential.conversations.length}`}`,
      `Operations: ${credential?.operations?.join(", ") ?? "account-wide"}`,
      `Directory: ${conversations.length} readable conversation(s)`,
      `This conversation: ${conversation.view} revision ${conversation.content_revision}, cursor ${cursor}, retained from ${conversation.unread?.oldest ?? "?"}`,
      `Pending tasks: ${pending} · socket ${this.options.socketReady() ? "connected" : "reconnecting"}`,
      `Config: model ${config.get("model.name")}, stream=${config.get("model.stream")}, autonomous=${config.get("autonomous")}, respondToAllDms=${config.get("chat.respondToAllDms")}, acknowledgeReads=${config.get("chat.acknowledgeReads")}`,
      `Uptime: ${uptime}s · syncs ${store.metrics.syncs} · edits ${store.metrics.edits} · retractions ${store.metrics.retractions} · sent ${store.metrics.sent} · replays ${store.metrics.replayed}`,
    ].join("\n");
  }

  private modelHistory(messages: Message[]): Message[] {
    const privateIds = new Set(
      this.options.store
        .tasks()
        .filter((task) => task.privateCommand)
        .flatMap((task) => [
          task.input,
          ...task.slots.map(
            (slot) =>
              this.options.store.part(
                `${taskKey(task.conversation, task.input)}:${slot}`,
              )?.response?.message.id ?? "",
          ),
        ]),
    );
    const maintenance = (text: string) =>
      /^(?:@\S+\s+)?\/(?:admin|features|status|model|config|reload|owner|ping|metrics|presence|schema|long|newroom|dm|read|help|whoami|history|react)\b/u.test(
        text,
      );
    return messages.filter(
      (recent) =>
        !privateIds.has(recent.id) &&
        !maintenance(recent.text) &&
        !(recent.reply && maintenance(recent.reply.text)),
    );
  }

  private async askModel(
    conversation: Conversation,
    message: Message,
    mode: "new" | "revised",
    task: TaskRecord,
    batch?: Message[],
    turnSignal?: AbortSignal,
  ): Promise<string> {
    const signal =
      turnSignal ??
      AbortSignal.any([
        this.abort.signal,
        ...(this.activeTask ? [this.activeTask.controller.signal] : []),
        AbortSignal.timeout(this.options.config.get("model.timeoutMs")),
      ]);
    return abortable(
      this.modelTurn(conversation, message, mode, task, signal, batch),
      signal,
    );
  }

  private async modelTurn(
    conversation: Conversation,
    message: Message,
    mode: "new" | "revised",
    task: TaskRecord,
    signal: AbortSignal,
    batch?: Message[],
  ): Promise<string> {
    const { api, config, model } = this.options;
    signal.throwIfAborted();
    this.options.log(
      "debug",
      `model preparation started conversation=${conversation.id} input=${message.id}`,
    );
    for (const action of task.actions ?? [])
      if (action.status === "pending")
        await this.recoverCommand(conversation.id, action);
    const commands = await this.serverCommands(conversation.id);
    this.options.log(
      "debug",
      `model command catalogue ready conversation=${conversation.id} input=${message.id} commands=${commands.length}`,
    );
    const historyEnabled = config.get("chat.contextChars") > 0;
    const readsEnabled =
      config.get("chat.acknowledgeReads") &&
      (this.snapshot?.credential.operations === null ||
        (this.snapshot?.credential.operations.includes("read_ack") ?? false));
    const before = batch
      ? Math.max(
          conversation.unread?.through ?? 0,
          ...batch.map((input) => input.sequence),
        ) + 1
      : message.sequence;
    const batchIds = new Set(batch?.map((input) => input.id) ?? []);
    const historyFilter = (messages: Message[]) =>
      this.modelHistory(messages).filter(
        (recent) => recent.sequence < before && !batchIds.has(recent.id),
      );
    const context = new ContextWindow(
      api,
      conversation.id,
      config.get("chat.contextChars"),
      historyFilter,
    );
    const prelude: ChatMessage[] = [];
    const metrics = this.metrics(conversation.id);
    let session = this.contexts.get(conversation.id);
    const newSession = session === undefined;
    if (!session) session = new AppendHistory();
    this.contexts.delete(conversation.id);
    this.contexts.set(conversation.id, session);
    if (this.contexts.size > 16)
      this.contexts.delete(this.contexts.keys().next().value!);
    const history = await session.prepare(
      api,
      conversation.id,
      before,
      config.get("chat.historyWindow"),
      config.get("chat.contextChars"),
      historyFilter,
    );
    metrics.contextChars = history.chars;
    metrics.contextMessages = history.rows.length;
    if (history.rebuilt) metrics.rebases++;
    this.options.log(
      "debug",
      `model history ready conversation=${conversation.id} input=${message.id} messages=${history.rows.length} chars=${history.chars} rebuilt=${history.rebuilt}`,
    );
    context.seed(history.rows, history.chars);
    for (const chunk of history.chunks)
      prelude.push({
        role: "user",
        content: `Conversation data [sequence,id,sender,text,offset,author_id]:\n${chunk}`,
      });
    const appendContext = async (
      query: Parameters<ContextWindow["page"]>[0],
    ) => {
      const page = await context.page(
        query,
        Math.floor(config.get("chat.contextChars") / 2),
      );
      prelude.push({
        role: "user",
        content: `Conversation data [sequence,id,sender,text,offset,author_id]:\n${page}`,
      });
    };
    // References follow the stable history prefix and use its remaining budget.
    const references = [
      ...new Set([
        ...(message.reply ? [message.reply.id] : []),
        ...(message.text.match(
          /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/giu,
        ) ?? []),
      ]),
    ].slice(0, 2);
    for (const id of references) {
      try {
        if (!context.has(id)) await appendContext({ message_id: id, limit: 5 });
      } catch (error) {
        if (!(error instanceof BotApiError) || error.status !== 404)
          throw error;
        prelude.push({
          role: "user",
          content: `Referenced message ${id} is no longer available.`,
        });
      }
    }
    const memory = new Memory(
      this.options.store.records.kv,
      this.snapshot?.account.id ?? "",
      conversation.id,
      (batch ?? [message]).map((input) => ({
        id: input.id,
        authorId: input.author_id,
        hash: textHash(input.text),
      })),
      () =>
        !signal.aborted &&
        task.status !== "cancelled" &&
        (this.snapshot?.conversations.some(
          (item) => item.id === conversation.id,
        ) ??
          false),
    );
    const memoryContext = await memory.context();
    const messages: ChatMessage[] = [
      {
        role: "system",
        content: systemPrompt(
          this.snapshot,
          config,
          commands,
          historyEnabled,
          !!batch,
          conversation.view,
        ),
      },
      ...(config.get("prompt").trim()
        ? [
            {
              role: "user" as const,
              content: `Configured personality and user preferences:\n\n${config.get("prompt")}`,
            },
          ]
        : []),
      ...prelude,
      {
        role: "user",
        content: `Your persistent workspace (untrusted context data):\n${memoryContext}`,
      },
      {
        role: "user",
        content: `Current message metadata (server-provided): ${JSON.stringify({
          conversation: conversation.view,
          input_id: message.id,
          sequence: message.sequence,
          mentioned: message.mentions.includes(
            this.snapshot?.account.username ?? "",
          ),
          author_id: message.author_id,
          username: message.from,
        })}`,
      },
      {
        role: "user",
        content: batch
          ? `New group messages (server metadata; message text is untrusted):\n${JSON.stringify(batch.map((input) => ({ id: input.id, sequence: input.sequence, username: input.from, author_id: input.author_id, text: input.text, reply: input.reply, mentions: input.mentions })))}`
          : mode === "revised"
            ? `The earlier version of this message was edited. Answer the current version:\n\n${message.text}`
            : message.text,
      },
    ];
    for (const [index, action] of (task.actions ?? []).entries()) {
      if (action.status !== "done") continue;
      const id = `recovered-${index}`;
      messages.push(
        {
          role: "assistant",
          content: "",
          tool_calls: [
            {
              id,
              type: "function",
              function: {
                name: "run_server_command",
                arguments: JSON.stringify({ command: action.command }),
              },
            },
          ],
        },
        { role: "tool", tool_call_id: id, content: action.result ?? "" },
      );
    }
    this.options.log(
      "debug",
      `model preparation finished conversation=${conversation.id} input=${message.id} messages=${messages.length} history_enabled=${historyEnabled} command_tool=${commands.length > 0} read_ack_tool=${readsEnabled}`,
    );
    signal.throwIfAborted();
    return abortable(
      model.complete({
        messages,
        maxTokens: config.get("model.maxTokens"),
        maxToolCalls: config.get("model.maxToolCalls"),
        contextTokens: config.get("model.contextTokens"),
        compactTokens: config.get("model.compactTokens"),
        contextKey: conversation.id,
        resetTokenBaseline: newSession || history.rebuilt,
        compact: (messages, target, tokensPerByte) => {
          const result = compactConversation(
            messages,
            undefined,
            target,
            tokensPerByte,
          );
          if (JSON.stringify(result) !== JSON.stringify(messages)) {
            this.contexts.delete(conversation.id);
            metrics.rebases++;
          }
          return result;
        },
        reasoningEffort: config.get("model.reasoningEffort"),
        signal,
        stopAfterTool: () => task.decision !== undefined,
        onCall: (call) => {
          metrics.record(call);
          const usage = call.usage;
          // A completed request's measured usage is authoritative. Start a fresh
          // retained-history baseline for the next answer when it reaches the
          // compaction threshold, even if the pre-request estimate was lower.
          if (
            usage?.prompt !== undefined &&
            usage.prompt >= config.get("model.compactTokens") &&
            this.contexts.delete(conversation.id)
          )
            metrics.rebases++;
          this.options.log(
            "debug",
            `model attempt conversation=${conversation.id} input=${message.id} success=${call.success} duration_ms=${Math.round(call.durationMs)} input_tokens=${usage?.prompt ?? "unknown"} output_tokens=${usage?.completion ?? "unknown"} cached_tokens=${usage?.cached ?? "unknown"} input_budget=${call.inputBudget?.tokens ?? "unknown"} budget_source=${call.inputBudget?.source ?? "unknown"}`,
          );
        },
        tools: modelTools({
          group: !!batch,
          commands: commands.length > 0,
          history: historyEnabled,
          reads: readsEnabled,
        }),
        executeTool: async (
          name: string,
          argumentsText: string,
        ): Promise<string> => {
          signal.throwIfAborted();
          if (name === "memory") return memory.execute(argumentsText);
          if (name === "execute_code") {
            if (
              task.status !== "running" ||
              this.stopped ||
              !this.snapshot?.conversations.some(
                (item) => item.id === conversation.id,
              )
            )
              return JSON.stringify({
                error: "Execution turn is no longer active",
              });
            return this.execution.execute(argumentsText, signal);
          }
          if (name === "finish_turn" && batch) {
            try {
              task.decision = groupResponses(
                argumentsText,
                batch,
                conversation.permissions.reply,
              );
            } catch (error) {
              return JSON.stringify({
                error:
                  error instanceof Error
                    ? error.message
                    : "Invalid response plan",
              });
            }
            await this.options.store.save();
            return JSON.stringify({
              finished: true,
              messages: task.decision.length,
            });
          }
          if (name === "read_history" && historyEnabled) {
            let input: unknown;
            try {
              input = JSON.parse(argumentsText);
            } catch {
              return JSON.stringify({ error: "Invalid arguments" });
            }
            if (
              typeof input !== "object" ||
              input === null ||
              Array.isArray(input) ||
              Object.entries(input).some(([key, value]) =>
                key === "message_id"
                  ? typeof value !== "string" ||
                    !/^[A-Za-z0-9._:-]{1,128}$/.test(value)
                  : !["before", "after", "around", "limit"].includes(key) ||
                    typeof value !== "number" ||
                    !Number.isSafeInteger(value) ||
                    value < (key === "after" ? 0 : 1) ||
                    (key === "limit" && value > 20),
              )
            )
              return JSON.stringify({
                error: "Expected bounded history cursor arguments",
              });
            const query = input as {
              before?: number;
              after?: number;
              around?: number;
              message_id?: string;
              limit?: number;
            };
            if (
              (query.message_id &&
                Object.keys(query).some(
                  (key) => !["message_id", "limit"].includes(key),
                )) ||
              [query.before, query.after, query.around].filter(
                (value) => value !== undefined,
              ).length > 1
            )
              return JSON.stringify({
                error: "Choose one cursor or message_id",
              });
            try {
              return await context.page({
                ...query,
                limit: query.limit ?? 6,
              });
            } catch (error) {
              if (
                error instanceof BotApiError &&
                [400, 404].includes(error.status)
              )
                return JSON.stringify({ error: error.message });
              throw error;
            }
          }
          if (name === "acknowledge_read" && readsEnabled) {
            let input: unknown;
            try {
              input = JSON.parse(argumentsText);
            } catch {
              return JSON.stringify({ error: "Invalid arguments" });
            }
            if (
              typeof input !== "object" ||
              input === null ||
              Array.isArray(input) ||
              Object.keys(input).length
            )
              return JSON.stringify({
                error: "Expected an empty argument object",
              });
            if (task.status === "cancelled" || this.abort.signal.aborted)
              throw new Error("Task cancelled before read acknowledgement");
            return JSON.stringify(
              await api.acknowledgeRead(conversation.id, message.sequence),
            );
          }
          if (name !== "run_server_command" || argumentsText.length > 8192)
            return JSON.stringify({
              error: "Unknown tool or oversized arguments",
            });
          let input: unknown;
          try {
            input = JSON.parse(argumentsText);
          } catch {
            return JSON.stringify({ error: "Invalid JSON arguments" });
          }
          if (
            typeof input !== "object" ||
            input === null ||
            Array.isArray(input) ||
            Object.keys(input).length !== 1 ||
            !("command" in input) ||
            typeof input.command !== "string"
          )
            return JSON.stringify({
              error: "Expected only a command string",
            });
          const command = input.command.trim();
          if (
            command.length > 4100 ||
            !commands.some((c) => c.name === command.split(/\s/u)[0])
          )
            return JSON.stringify({
              error: "Command is not in the available server catalogue",
            });
          if (/^(?:\/sudo\s+)?\/(?:reply|tell)(?:\s|$)/u.test(command))
            return JSON.stringify({
              error:
                "Return your answer as the final response; replies are sent once through the worker outbox",
            });
          if (/^(?:\/sudo\s+)?\/history(?:\s|$)/u.test(command))
            return JSON.stringify({
              error: "Use read_history for conversation history",
            });
          if (task.status === "cancelled" || this.abort.signal.aborted)
            throw new Error("Task was cancelled before command execution");
          let action = task.actions?.find(
            (action) => action.command === command,
          );
          if (!action) {
            if ((task.actions?.length ?? 0) >= 16)
              return JSON.stringify({
                error: "Task action limit reached",
              });
            action = {
              command,
              status: "pending",
              key: idempotencyKey(
                conversation.id,
                message.id,
                `command:${task.actions?.length ?? 0}`,
              ),
              created: Date.now(),
            };
            (task.actions ??= []).push(action);
            action = task.actions.at(-1)!;
            await this.options.store.save();
            return this.executeRecordedCommand(conversation.id, action);
          }
          if (action.status === "pending")
            await this.recoverCommand(conversation.id, action);
          return action.result ?? "";
        },
      }),
      signal,
    );
  }

  private async executeRecordedCommand(
    conversation: string,
    action: CommandAction,
  ): Promise<string> {
    if (
      !action.key ||
      action.created === undefined ||
      Date.now() - action.created >= 23 * 60 * 60 * 1000
    )
      throw new Error(
        "Command receipt retry window expired or legacy command is unkeyed; inspect before retrying",
      );
    try {
      const value = await this.options.api.executeCommand(
        conversation,
        action.command,
        action.key,
      );
      action.result = JSON.stringify({ result: preview(value.result, 3000) });
    } catch (error) {
      if (
        !(error instanceof BotApiError) ||
        error.retryable ||
        error.code === "network"
      )
        throw error;
      action.result = JSON.stringify({ error: preview(error.message, 500) });
    }
    action.status = "done";
    await this.options.store.save();
    return action.result;
  }
  private async recoverCommand(
    conversation: string,
    action: CommandAction,
    submitMissing = true,
  ): Promise<void> {
    if (!action.key)
      throw new Error("Legacy command is unkeyed; inspect before retrying");
    const receipt = await this.options.api.commandReceipt(action.key);
    if (receipt.state === "not_found") {
      if (submitMissing)
        await this.executeRecordedCommand(conversation, action);
      return;
    }
    action.result = JSON.stringify(
      receipt.state === "succeeded"
        ? { result: preview(receipt.result ?? "", 3000) }
        : { error: receipt.error?.message ?? "Command failed" },
    );
    action.status = "done";
    await this.options.store.save();
  }

  // ------------------------------------------------------------------ output

  /** Send text, splitting long output; every part is durable before it is sent. */
  private async sendParts(
    conversation: Conversation,
    task: TaskRecord,
    slot: string,
    text: string,
  ): Promise<void> {
    this.stageParts(conversation, task, slot, text, task.replyTo ?? undefined);
    await this.options.store.save();
    await this.deliverParts(conversation, task, slot);
  }

  private stageParts(
    conversation: Conversation,
    task: TaskRecord,
    slot: string,
    text: string,
    replyTo?: string,
  ): void {
    const { store } = this.options;
    const parts = chunkMessage(text);
    const numbered = parts.length > 1;
    const key = taskKey(conversation.id, task.input);
    for (const [index, body] of parts.entries()) {
      const name = numbered ? `${slot}:${index + 1}` : slot;
      const full = `${key}:${name}`;
      const labelled = numbered
        ? `[${index + 1}/${parts.length}] ${body}`
        : body;
      let part = store.part(full);
      if (part === undefined) {
        part = {
          slot: full,
          key: idempotencyKey(conversation.id, task.input, name),
          request: {
            conversation_id: conversation.id,
            text: labelled,
            ...(replyTo === undefined ? {} : { reply_to: replyTo }),
            markdown: true,
          },
          created: Date.now(),
        };
        store.putPart(part);
        if (!task.slots.includes(name)) task.slots.push(name);
        store.putTask(task);
      }
    }
  }

  private async deliverParts(
    conversation: Conversation,
    task: TaskRecord,
    slot: string,
    signal?: AbortSignal,
  ): Promise<boolean> {
    const { store } = this.options;
    const key = taskKey(conversation.id, task.input);
    const parts = task.slots
      .filter((name) => name === slot || name.startsWith(`${slot}:`))
      .map((name) => store.part(`${key}:${name}`));
    if (parts.length === 0) return false;
    for (const part of parts) {
      signal?.throwIfAborted();
      if (this.stopped) throw new Error("worker stopped for transport change");
      if (part === undefined)
        throw new Error("checkpoint output batch is incomplete");
      if (task.status === "cancelled") return true;
      if (part.response !== undefined) continue;
      if (Date.now() - part.created >= 23 * 60 * 60 * 1000) {
        throw new Error(
          "pending send is near receipt expiry; inspect history before resolving it",
        );
      }
      const response = await this.options.api.send(part.request, part.key);
      part.response = {
        message: {
          id: response.message.id,
          sequence: response.message.sequence,
        },
        replayed: response.replayed,
      };
      store.putPart(part);
      if (response.replayed) store.metrics.replayed++;
      else store.metrics.sent++;
      await store.save();
    }
    return true;
  }

  // ---------------------------------------------------------------- activity

  private beginActivity(conversationId: string): void {
    if (
      this.activityConversation !== null &&
      this.activityConversation !== conversationId
    ) {
      this.endActivity();
    }
    this.activityConversation = conversationId;
    this.options.setActivity(conversationId, "working");
    // The server expires a working signal after 30 s; refresh well inside that.
    this.activityTimer ??= setInterval(
      () =>
        this.options.setActivity(
          this.activityConversation ?? conversationId,
          "working",
        ),
      20_000,
    );
  }

  /** Idle must name the conversation that was marked working; the server rejects an empty id. */
  private endActivity(): void {
    if (this.activityTimer !== null) {
      clearInterval(this.activityTimer);
      this.activityTimer = null;
    }
    const conversation = this.activityConversation;
    this.activityConversation = null;
    if (conversation !== null) this.options.setActivity(conversation, "idle");
  }

  // ------------------------------------------------------------------ helpers

  private async history(
    query: Parameters<BotApi["history"]>[0],
  ): Promise<HistoryResponse> {
    return this.options.api.history(query);
  }
}

export function preview(text: string, limit: number): string {
  const single = text.replace(/\s+/g, " ").trim();
  const characters = Array.from(single);
  return characters.length <= limit
    ? single
    : `${characters.slice(0, limit).join("")}…`;
}
