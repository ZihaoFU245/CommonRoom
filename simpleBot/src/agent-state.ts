import { Shell } from "./shell.ts";
import { Execution } from "./execution.ts";
import type { ConfigStore } from "./config.ts";
import type { BotApi } from "./api.ts";
import type { Store } from "./store.ts";
import type { ChatModel } from "./model.ts";
import type { Logger } from "./log.ts";
import type { Snapshot } from "./wire.ts";
import type { AppendHistory } from "./context.ts";
import { Metrics } from "./metrics.ts";

export interface AgentOptions {
  api: BotApi;
  store: Store;
  config: ConfigStore;
  model: ChatModel;
  workspace: string;
  readonly withShell?: boolean;
  log: Logger;
  presence(): string[];
  socketReady(): boolean;
  setActivity(conversationId: string, state: "working" | "idle"): void;
}

/** Shared worker services. Scheduling and effects have separate owners. */
export class AgentState {
  readonly options: AgentOptions;
  snapshot: Snapshot | null = null;
  stopped = false;
  readonly startedAt = Date.now();
  readonly abort = new AbortController();
  readonly execution: Execution;
  readonly shell: Shell;
  readonly contexts = new Map<string, AppendHistory>();
  readonly modelMetrics = new Map<string, Metrics>();
  private readonly activity = new Map<string, ReturnType<typeof setInterval>>();

  constructor(options: AgentOptions) {
    this.options = options;
    this.execution = new Execution(options.workspace);
    this.shell = new Shell(options.workspace, options.withShell === true);
  }

  metrics(conversation: string): Metrics {
    let value = this.modelMetrics.get(conversation);
    if (!value) value = new Metrics();
    this.modelMetrics.delete(conversation);
    this.modelMetrics.set(conversation, value);
    if (this.modelMetrics.size > 64)
      this.modelMetrics.delete(this.modelMetrics.keys().next().value!);
    return value;
  }

  beginActivity(id: string): void {
    if (this.activity.has(id)) return;
    this.options.setActivity(id, "working");
    this.activity.set(
      id,
      setInterval(() => this.options.setActivity(id, "working"), 20000),
    );
  }

  endActivity(id?: string): void {
    for (const [conversation, timer] of this.activity) {
      if (id !== undefined && id !== conversation) continue;
      clearInterval(timer);
      this.activity.delete(conversation);
      this.options.setActivity(conversation, "idle");
    }
  }
}
