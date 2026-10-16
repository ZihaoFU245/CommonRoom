// Transport convergence and input admission. Inference lives in AgentLoop.
import { AgentState, type AgentOptions } from "./agent-state.ts";
export type { AgentOptions } from "./agent-state.ts";
export { preview } from "./text.ts";
import { InputSync } from "./agent-input.ts";
import { AgentLoop } from "./agent-loop.ts";
import { Controls } from "./agent-controls.ts";
import { ModelContext } from "./agent-model.ts";
import { Outbox } from "./outbox.ts";
import type { BotApi } from "./api.ts";
import type { ChatModel } from "./model.ts";
import type { Snapshot, HistoryResponse, BotCommand } from "./wire.ts";

export class Agent extends AgentState {
  private syncing = false;
  private dirty = false;
  private drainRequested = false;
  private readonly loop: AgentLoop;
  private readonly controls: Controls;
  private readonly modelContext: ModelContext;
  private readonly outbox: Outbox;
  private readonly input: InputSync;

  constructor(options: AgentOptions) {
    super(options);
    this.controls = new Controls(this);
    this.modelContext = new ModelContext(this);
    this.outbox = new Outbox(this);
    this.loop = new AgentLoop(this, this.modelContext, this.outbox, () => {
      if (!this.input.blocked || this.stopped) return;
      this.requestSync(true, false).catch((error) =>
        this.options.log(
          "warn",
          `input catch-up deferred: ${error instanceof Error ? error.message : "sync failure"}`,
        ),
      );
    });
    this.input = new InputSync(
      this,
      this.loop,
      this.controls,
      this.modelContext,
    );
  }
  get directory(): Snapshot | null {
    return this.snapshot;
  }
  get busy(): boolean {
    return this.syncing || this.loop.busy;
  }
  setModel(model: ChatModel): void {
    this.options.model = model;
  }
  commands(conversation: string): Promise<BotCommand[]> {
    return this.controls.commands(conversation);
  }
  operatorValues(): Record<string, string> {
    return this.controls.operatorValues();
  }
  async stop(): Promise<void> {
    this.stopped = true;
    this.abort.abort();
    this.endActivity();
    await this.loop.close();
    while (this.syncing)
      await new Promise((resolve) => setTimeout(resolve, 10));
    await this.execution.settle();
    await this.options.store.save();
  }
  /** Apply a snapshot and converge on the server's retained state. */
  async handleSnapshot(snapshot: Snapshot, settleWork = true): Promise<void> {
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
    await this.requestSync(true, settleWork);
  }

  /**
   * Converge and handle local controls without scheduling model work. Socket
   * notifications use requestSync(true, false) and never await inference.
   */
  async sync(): Promise<void> {
    await this.requestSync(false);
  }

  /**
   * Coalesce signals: at most one history sync runs, with one rerun queued.
   * Local controls bypass inference. settleWork is for callers that explicitly
   * want currently runnable ticks/delivery to finish; event sources pass false.
   */
  async requestSync(drainWork = true, settleWork = true): Promise<void> {
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
        const rerun = await this.input.syncOnce();
        this.dirty ||= rerun;
      } while (this.dirty && !this.stopped);
    } finally {
      this.syncing = false;
    }
    await this.admitControls();
    if (this.drainRequested && !this.stopped) {
      this.drainRequested = false;
      for (const conversation of this.snapshot?.conversations ?? [])
        this.loop.wake(conversation.id);
      if (settleWork) await this.loop.settle();
    }
  }

  private async admitControls(): Promise<void> {
    const { store, config } = this.options;
    for (const task of store.tasks()) {
      if (task.status !== "queued") continue;
      const conversation = this.snapshot?.conversations.find(
        (item) => item.id === task.conversation,
      );
      if (!conversation) continue;
      if (
        !config.get("autonomous") &&
        task.autonomous &&
        !task.slots.length &&
        !task.actions?.length
      ) {
        task.status = "cancelled";
        continue;
      }
      const page = await this.history({
        conversation_id: conversation.id,
        around: task.sequence,
        limit: 1,
      });
      const message = page.messages.find((item) => item.id === task.input);
      if (!message || !this.controls.recognizes(conversation, message))
        continue;
      task.status = "running";
      this.beginActivity(conversation.id);
      await store.save();
      try {
        const response = await this.controls.handle(
          conversation,
          message,
          task,
        );
        if (response !== undefined)
          await this.outbox.sendParts(
            conversation,
            task,
            task.outputSlot ?? "answer",
            response,
          );
        task.status = "answered";
      } catch (error) {
        task.status = "failed";
        task.error = error instanceof Error ? error.message : "Control failed";
      }
      this.endActivity(conversation.id);
      task.updated = Date.now();
      await store.save();
    }
    await store.save();
  }
  private history(
    query: Parameters<BotApi["history"]>[0],
  ): Promise<HistoryResponse> {
    return this.options.api.history(query);
  }
}
