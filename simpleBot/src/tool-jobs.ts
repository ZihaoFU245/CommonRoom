import { setTimeout as wait } from "node:timers/promises";
import { abortable } from "./async.ts";
import type { FunctionTool } from "./model.ts";
import { ToolLocks, type ToolAccess } from "./tool-locks.ts";

export const WAIT_JOBS_TOOL: FunctionTool = {
  type: "function",
  function: {
    name: "wait_jobs",
    description:
      "Yield until the selected background tool jobs finish and collect their results. Omit job_ids to wait for all outstanding jobs. Use when you need a result; do independent work first. Jobs belong to this turn only. Do not repeat a tool to check its status.",
    parameters: {
      type: "object",
      properties: {
        job_ids: {
          type: "array",
          items: { type: "string" },
          minItems: 1,
          maxItems: 128,
          uniqueItems: true,
        },
      },
      additionalProperties: false,
    },
  },
};

interface Job {
  id: string;
  tool: string;
  status: "queued" | "running" | "completed" | "failed";
  result?: string;
  delivered: boolean;
  done: Promise<void>;
}

export interface JobResult {
  job_id: string;
  tool: string;
  status: Job["status"];
  /** Tool output is untrusted data, including error text. */
  result?: string;
}

/** Turn-local jobs; the shared lock manager also excludes other completions. */
export class ToolJobs {
  private readonly jobs = new Map<string, Job>();
  private readonly controller = new AbortController();
  readonly signal: AbortSignal;
  private readonly execute: (
    name: string,
    args: string,
    signal: AbortSignal,
  ) => Promise<string>;
  private readonly access: (name: string, args: string) => ToolAccess;
  private readonly locks: ToolLocks;

  constructor(
    execute: (
      name: string,
      args: string,
      signal: AbortSignal,
    ) => Promise<string>,
    access: (name: string, args: string) => ToolAccess,
    locks: ToolLocks,
    signal: AbortSignal,
  ) {
    this.execute = execute;
    this.access = access;
    this.locks = locks;
    this.signal = AbortSignal.any([signal, this.controller.signal]);
  }

  get outstanding(): boolean {
    return [...this.jobs.values()].some((job) => !job.delivered);
  }

  async dispatch(name: string, args: string): Promise<string> {
    this.signal.throwIfAborted();
    const job: Job = {
      id: `job-${this.jobs.size + 1}`,
      tool: name,
      status: "queued",
      delivered: false,
      done: Promise.resolve(),
    };
    this.jobs.set(job.id, job);
    job.done = this.locks
      .run(this.access(name, args), this.signal, async () => {
        job.status = "running";
        job.result = await this.execute(name, args, this.signal);
        job.status = "completed";
      })
      .catch((error: unknown) => {
        job.status = "failed";
        job.result = JSON.stringify({
          error: this.signal.aborted
            ? "Tool job cancelled; effects may already have occurred"
            : error instanceof Error
              ? error.message
              : "Tool job failed",
        });
      });
    // Fast KV calls keep their ordinary result; slow calls return control.
    const grace = new AbortController();
    try {
      await abortable(
        Promise.race([
          job.done,
          wait(10, undefined, { signal: grace.signal }).catch(() => {}),
        ]),
        this.signal,
      );
    } finally {
      grace.abort();
    }
    this.signal.throwIfAborted();
    if (job.status === "completed" || job.status === "failed") {
      job.delivered = true;
      return job.result ?? "";
    }
    return JSON.stringify(this.result(job));
  }

  /** Automatic completion notices, emitted once before the next model step. */
  collect(): JobResult[] {
    return [...this.jobs.values()]
      .filter(
        (job) =>
          !job.delivered &&
          (job.status === "completed" || job.status === "failed"),
      )
      .map((job) => {
        job.delivered = true;
        return this.result(job);
      });
  }

  async wait(raw = "{}"): Promise<string> {
    return JSON.stringify({ jobs: await this.waitResults(raw) });
  }

  async waitResults(raw = "{}"): Promise<JobResult[]> {
    const input: unknown = JSON.parse(raw);
    if (
      typeof input !== "object" ||
      input === null ||
      Array.isArray(input) ||
      Object.keys(input).some((key) => key !== "job_ids")
    )
      throw new Error("Expected optional job_ids only");
    const ids = "job_ids" in input ? input.job_ids : undefined;
    if (
      ids !== undefined &&
      (!Array.isArray(ids) ||
        ids.length < 1 ||
        ids.length > 128 ||
        ids.some((id) => typeof id !== "string" || !this.jobs.has(id)) ||
        new Set(ids).size !== ids.length)
    )
      throw new Error("Expected unique job IDs from this turn");
    const selected = [...this.jobs.values()].filter((job) =>
      Array.isArray(ids) ? ids.includes(job.id) : !job.delivered,
    );
    await abortable(Promise.all(selected.map((job) => job.done)), this.signal);
    return selected.map((job) => {
      job.delivered = true;
      return this.result(job);
    });
  }

  async close(): Promise<void> {
    this.controller.abort(new Error("Tool turn ended"));
    // Cooperative tools (notably subprocesses) finish cleanup before returning.
    await Promise.all([...this.jobs.values()].map((job) => job.done));
  }

  private result(job: Job): JobResult {
    return {
      job_id: job.id,
      tool: job.tool,
      status: job.status,
      ...(job.result === undefined ? {} : { result: job.result }),
    };
  }
}
