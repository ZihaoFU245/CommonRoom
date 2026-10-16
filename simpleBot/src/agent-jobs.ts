import type { AgentState } from "./agent-state.ts";
import type { CompletionRequest } from "./model.ts";
import { ToolLocks } from "./tool-locks.ts";
import { toolAccess } from "./tools.ts";
import { Scheduler } from "./scheduler.ts";
import { abortable } from "./async.ts";
import type { TickJob, TickSession } from "./tick.ts";

/** Worker-owned jobs and resource locks; model request completion cannot cancel them. */
export class AgentJobs {
  private readonly host: AgentState;
  private readonly locks = new ToolLocks();
  private readonly scheduler: Scheduler;
  private readonly work = new Map<string, () => Promise<void>>();
  private readonly controllers = new Map<
    string,
    { conversation: string; controller: AbortController }
  >();
  private readonly completed: (conversation: string) => void;

  constructor(host: AgentState, completed: (conversation: string) => void) {
    this.host = host;
    this.completed = completed;
    this.scheduler = new Scheduler(
      async (id) => {
        const run = this.work.get(id);
        this.work.delete(id);
        await run?.();
      },
      () => host.options.config.get("agent.maxConcurrentJobs"),
      (error) =>
        host.options.log(
          "error",
          `job checkpoint failed: ${error instanceof Error ? error.message : "storage failure"}`,
        ),
    );
  }

  start(
    conversation: string,
    session: TickSession,
    job: TickJob,
    request: CompletionRequest,
    signal: AbortSignal,
  ): void {
    const controller = new AbortController();
    const episodeRemaining =
      session.started +
      this.host.options.config.get("agent.episodeTimeoutMs") -
      Date.now();
    const activeSignal = AbortSignal.any([
      controller.signal,
      signal,
      this.host.abort.signal,
      AbortSignal.timeout(
        Math.max(
          1,
          Math.min(
            this.host.options.config.get("model.timeoutMs"),
            episodeRemaining,
          ),
        ),
      ),
    ]);
    this.controllers.set(job.id, { conversation, controller });
    this.work.set(job.id, async () => {
      try {
        activeSignal.throwIfAborted();
        job.status = "running";
        await this.host.options.store.save();
        const result = await this.locks.run(
          toolAccess(
            job.tool,
            job.arguments,
            conversation,
            this.host.execution.workspace,
          ),
          activeSignal,
          async () => {
            if (!request.executeTool)
              throw new Error("Unavailable tool executor");
            // Keep the resource lock until a cooperative implementation cleans up.
            return request.executeTool(
              job.tool,
              job.arguments,
              activeSignal,
              job.id,
            );
          },
        );
        activeSignal.throwIfAborted();
        job.result =
          result.length > 64000
            ? JSON.stringify({ truncated: true, text: result.slice(0, 60000) })
            : result;
        job.status = "completed";
      } catch (error) {
        job.status = activeSignal.aborted ? "interrupted" : "failed";
        job.result = JSON.stringify({
          error: activeSignal.aborted
            ? "Job interrupted; effects may already have occurred"
            : error instanceof Error
              ? error.message.slice(0, 1000)
              : "Job failed",
        });
      } finally {
        this.controllers.delete(job.id);
        await this.host.options.store.save();
        this.completed(conversation);
      }
    });
    this.scheduler.wake(job.id);
  }

  cancel(conversation: string): void {
    for (const value of this.controllers.values())
      if (value.conversation === conversation) value.controller.abort();
  }

  async close(): Promise<void> {
    for (const value of this.controllers.values()) value.controller.abort();
    // Let queued jobs record interruption instead of dropping their durable intent.
    await abortable(this.scheduler.settle(), AbortSignal.timeout(10000));
    await this.scheduler.close();
  }
}
