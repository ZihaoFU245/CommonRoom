import type { AgentState } from "./agent-state.ts";
import type { AgentLoop } from "./agent-loop.ts";
import type { Controls } from "./agent-controls.ts";
import type { ModelContext } from "./agent-model.ts";
import { BotApiError, type BotApi } from "./api.ts";
import { abortable } from "./async.ts";
import { taskKey, textHash } from "./store.ts";
import type {
  Conversation,
  Message,
  HistoryResponse,
  Snapshot,
} from "./wire.ts";

/** Authoritative history convergence, version reconciliation and input admission. */
export class InputSync {
  blocked = false;
  private dirty = false;
  private readonly retainedBaselines = new Map<
    string,
    { revision: number; oldest: number }
  >();
  private readonly host: AgentState;
  private readonly loop: AgentLoop;
  private readonly controls: Controls;
  private readonly modelContext: ModelContext;
  constructor(
    host: AgentState,
    loop: AgentLoop,
    controls: Controls,
    context: ModelContext,
  ) {
    this.host = host;
    this.loop = loop;
    this.controls = controls;
    this.modelContext = context;
  }
  async syncOnce(baseline?: Snapshot): Promise<boolean> {
    const { api, store, log } = {
      api: this.host.options.api,
      store: this.host.options.store,
      log: this.host.options.log,
    };
    const snapshot = baseline ?? (await api.me());
    this.blocked = false;
    let save = store.state.account === null;
    if (
      store.state.account !== null &&
      store.state.account !== snapshot.account.id
    ) {
      throw new Error("checkpoint belongs to another account");
    }
    store.state.account = snapshot.account.id;
    this.host.snapshot = snapshot;
    this.modelContext.warmCatalogues();
    store.metrics.syncs++;

    const live = new Set(
      snapshot.conversations.map((conversation) => conversation.id),
    );
    for (const id of this.host.contexts.keys())
      if (!live.has(id)) this.host.contexts.delete(id);
    for (const id of this.host.modelMetrics.keys())
      if (!live.has(id)) this.host.modelMetrics.delete(id);
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
            this.modelContext.recoverCommand(task.conversation, action, false),
            AbortSignal.timeout(3000),
          );
        } catch (error) {
          recovered = false;
          task.error =
            error instanceof Error ? error.message : "Command recovery failed";
          log("warn", `recovery deferred for ${task.input}: ${task.error}`);
        }
      }
      for (const action of task.actions ?? []) {
        if (!action.effectId || action.status !== "done") continue;
        const job =
          store.state.sessions[task.conversation]?.jobs[action.effectId];
        if (job && (job.status === "interrupted" || job.status === "failed")) {
          job.status = "completed";
          job.result = action.result;
          save = true;
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
    for (const id of Object.keys(store.state.sessions))
      if (!live.has(id)) this.loop.invalidate(id);
    for (const conversation of snapshot.conversations)
      if (!conversation.permissions.send) this.loop.invalidate(conversation.id);
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
        this.retainedBaselines.set(conversation.id, {
          revision: conversation.content_revision,
          oldest: conversation.unread.oldest,
        });
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
      let prefetched: Message[] | undefined;
      if (
        previous !== undefined &&
        previous !== conversation.content_revision
      ) {
        prefetched = await this.appendOnly(conversation, previous);
        if (prefetched === undefined)
          prefetched = await this.reconcile(conversation, previous);
        else {
          store.setRevision(conversation.id, conversation.content_revision);
          save = true;
        }
      } else if (previous === undefined) {
        store.setRevision(conversation.id, conversation.content_revision);
        save = true;
      }
      await this.advance(conversation, prefetched);
      if (store.revision(conversation.id) === conversation.content_revision) {
        this.retainedBaselines.delete(conversation.id);
        this.retainedBaselines.set(conversation.id, {
          revision: conversation.content_revision,
          oldest: conversation.unread.oldest,
        });
        if (this.retainedBaselines.size > 256)
          this.retainedBaselines.delete(
            this.retainedBaselines.keys().next().value!,
          );
      }
    }
    if (save) await store.save();
    const rerun = this.dirty;
    this.dirty = false;
    return rerun;
  }

  /** Every insertion and every other content mutation increments revision.
   * Equal revision delta and appended-row count proves an append-only interval,
   * provided retention did not move. Never infer this from sequence gaps. */
  private async appendOnly(
    conversation: Conversation,
    previous: number,
  ): Promise<Message[] | undefined> {
    const baseline = this.retainedBaselines.get(conversation.id);
    const cursor = this.host.options.store.cursor(conversation.id) ?? 0;
    const delta = conversation.content_revision - previous;
    if (
      !baseline ||
      baseline.revision !== previous ||
      !conversation.unread ||
      baseline.oldest !== conversation.unread.oldest ||
      delta <= 0 ||
      delta > 1000 ||
      conversation.unread.through <= cursor
    )
      return undefined;
    const messages: Message[] = [];
    let after = cursor;
    for (;;) {
      let page: HistoryResponse;
      try {
        page = await this.history({
          conversation_id: conversation.id,
          after,
          limit: 100,
          content_revision: conversation.content_revision,
        });
      } catch (error) {
        if (error instanceof BotApiError && error.code === "conflict")
          return undefined;
        throw error;
      }
      messages.push(...page.messages);
      if (messages.length > delta) return undefined;
      const last = page.messages.at(-1)?.sequence;
      if (!page.has_after || last === undefined) break;
      if (last <= after) throw new Error("Append history did not advance");
      after = last;
    }
    return messages.length === delta ? messages : undefined;
  }

  /**
   * Read new input after the saved cursor.
   *
   * Persist input acceptance and its scan cursor together. Inference and
   * delivery have independent checkpoints, so slow work cannot pin scanning.
   * A crash resumes accepted tasks instead of refetching them as new work.
   */
  private async advance(
    conversation: Conversation,
    prefetched?: Message[],
  ): Promise<void> {
    const { store } = this.host.options;
    const accountId = this.host.snapshot?.account.id;
    let after = store.cursor(conversation.id) ?? 0;
    for (;;) {
      const page =
        prefetched === undefined
          ? await this.history({
              conversation_id: conversation.id,
              after,
              limit: 100,
            })
          : { messages: prefetched, has_after: false };
      this.host.options.log(
        "debug",
        `input history page conversation=${conversation.id} after=${after} messages=${page.messages.length} has_after=${page.has_after}`,
      );
      if (page.messages.length === 0) return;
      for (const message of page.messages) {
        if (message.sequence <= (store.cursor(conversation.id) ?? 0)) continue;

        // Our own messages are recorded so they are never re-scanned, but they
        // never become work and never block the cursor.
        if (message.author_id !== accountId) {
          if (!this.consider(conversation, message)) {
            this.blocked = true;
            return;
          }
        } else
          this.host.options.log(
            "debug",
            `input skipped conversation=${conversation.id} input=${message.id} sequence=${message.sequence} reason=own_message`,
          );
        store.remember(
          conversation.id,
          message.id,
          textHash(message.text),
          message.sequence,
        );
        store.setCursor(conversation.id, message.sequence);
        after = Math.max(after, message.sequence);
        // Acceptance and the scan watermark publish in the same transaction.
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
  ): Promise<Message[] | undefined> {
    const { store, log } = this.host.options;
    let admitted: Message[] | undefined = [];
    const cursor = store.cursor(conversation.id) ?? 0;
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
        if (message.sequence > cursor && admitted) {
          admitted.push(message);
          if (admitted.length > 1000) admitted = undefined;
        }
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
    return admitted;
  }

  /**
   * An input we already answered changed. Update the recorded version in place
   * and let the scheduler pick it up again; delivered parts stay in the
   * transcript so the revision pass is additive rather than a correction.
   */
  private replaceTask(
    conversation: Conversation,
    messageId: string,
    hash: string,
  ): void {
    const store = this.host.options.store;
    this.loop.invalidate(conversation.id, messageId);
    const task = store.task(conversation.id, messageId);
    if (task === undefined) {
      // An edit of a message this worker never processed: nothing to revise.
      return;
    }
    const chat = this.host.snapshot?.conversations.find(
      (candidate) => candidate.id === conversation.id,
    );
    if (task.autonomous && task.status === "answered") {
      delete task.batch;
      delete task.decision;
      task.readyAt =
        Date.now() + this.host.options.config.get("chat.batchWindowMs");
    }
    if (task.outputHash !== hash) {
      task.pass = (task.pass ?? 0) + 1;
      task.outputSlot = `edit:${task.pass}`;
    }
    delete task.silentHash;
    task.promptHash = hash;
    task.replyTo = chat?.permissions.reply ? messageId : null;
    task.status = "queued";
    task.updated = Date.now();
    store.putTask(task);
    this.host.metrics(conversation.id).admit(messageId);
  }

  /** A pending input vanished: stop its unsent output, keep what was delivered. */
  private cancelTask(conversation: Conversation, messageId: string): void {
    const store = this.host.options.store;
    this.loop.invalidate(conversation.id, messageId);
    const task = store.task(conversation.id, messageId);
    if (task === undefined) return;
    const key = taskKey(conversation.id, messageId);
    const unsent = task.slots.filter(
      (slot) => store.part(`${key}:${slot}`)?.response === undefined,
    );
    store.deleteParts(unsent.map((slot) => `${key}:${slot}`));
    task.slots = task.slots.filter((slot) => !unsent.includes(slot));
    task.status = "cancelled";
    task.updated = Date.now();
    this.host.metrics(conversation.id).forgetInput(messageId);
  }

  /** Decide whether an input deserves an answer, then record that decision. */
  private consider(conversation: Conversation, message: Message): boolean {
    const account = this.host.snapshot?.account;
    if (account === undefined || message.author_id === account.id) return true;
    const autonomous =
      this.host.options.config.get("autonomous") &&
      !conversation.view.startsWith("@");
    const addressed =
      autonomous ||
      message.mentions.includes(account.username) ||
      (conversation.view.startsWith("@direct:") &&
        this.host.options.config.get("chat.respondToAllDms"));
    const existing = this.host.options.store.task(conversation.id, message.id);
    if (!addressed || !conversation.permissions.send || existing) {
      this.host.options.log(
        "debug",
        `input skipped conversation=${conversation.id} input=${message.id} sequence=${message.sequence} reason=${!addressed ? "not_addressed" : !conversation.permissions.send ? "send_not_permitted" : "existing_task"}${existing ? ` status=${existing.status}` : ""}`,
      );
      return true;
    }
    if (
      this.host.options.store
        .tasks()
        .filter((task) => task.status === "queued" || task.status === "running")
        .length >= this.host.options.config.get("agent.inboxLimit")
    ) {
      this.host.options.log(
        "warn",
        `input admission deferred conversation=${conversation.id} reason=inbox_full`,
      );
      return false;
    }
    const immediate = message.mentions.includes(account.username);
    const grouped =
      autonomous && !this.controls.recognizes(conversation, message);
    const readyAt =
      this.host.options.store
        .tasks()
        .find(
          (task) =>
            task.conversation === conversation.id &&
            task.autonomous &&
            task.status === "queued",
        )?.readyAt ??
      Date.now() + this.host.options.config.get("chat.batchWindowMs");
    this.host.options.store.putTask({
      ...(grouped
        ? {
            autonomous: true,
            readyAt: immediate ? Date.now() : readyAt,
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
    this.host.metrics(conversation.id).admit(message.id);
    this.host.options.log(
      "debug",
      `task queued conversation=${conversation.id} input=${message.id} sequence=${message.sequence}`,
    );
    return true;
  }
  private history(
    query: Parameters<BotApi["history"]>[0],
  ): Promise<HistoryResponse> {
    return this.host.options.api.history(query);
  }
}
