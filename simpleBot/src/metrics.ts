import type { ModelCall } from "./model.ts";
import type { TokenCount } from "./tokens.ts";

type Stage =
  | "Queue (includes batching)"
  | "Source validation"
  | "Context preparation"
  | "Model attempt"
  | "First streamed delta"
  | "Decision commit"
  | "Delivery queue"
  | "Delivery"
  | "End-to-end reply";
interface Timing {
  count: number;
  total: number;
  latest: number;
  recent: number[];
}
interface InputTiming {
  admitted: number;
  dequeued: boolean;
  calls?: number;
  planned?: boolean;
}
interface ReplyTiming {
  pending: Set<string>;
  inputs: Map<string, InputTiming>;
  committed: number;
  started?: number;
}
export interface MetricLimits {
  compactTokens: number;
  contextTokens: number;
  batchWindowMs: number;
}

/** In-memory, per-conversation diagnostics. Never retains prompts or responses. */
export class Metrics {
  private readonly timings = new Map<Stage, Timing>();
  private readonly inputs = new Map<string, InputTiming>();
  private readonly replies = new Map<string, ReplyTiming>();
  validationInputs = 0;
  validationRequests = 0;
  private replyCallTotal = 0;
  private replyCallReports = 0;
  attempts = 0;
  successes = 0;
  failures = 0;
  durationMs = 0;
  preparations = 0;
  preparationMs = 0;
  firstDeltaMs = 0;
  firstDeltaReports = 0;
  prompt = 0;
  promptReports = 0;
  completion = 0;
  completionReports = 0;
  cached = 0;
  cacheReports = 0;
  cachePrompt = 0;
  ratioReports = 0;
  rebases = 0;
  contextChars = 0;
  contextMessages = 0;
  latestInput: number | undefined;
  latestOutput: number | undefined;
  latestBudget: TokenCount | undefined;

  private time(stage: Stage, durationMs: number): void {
    const value = this.timings.get(stage) ?? {
      count: 0,
      total: 0,
      latest: 0,
      recent: [],
    };
    value.count++;
    value.total += Math.max(0, durationMs);
    value.latest = Math.max(0, durationMs);
    value.recent.push(value.latest);
    if (value.recent.length > 128) value.recent.shift();
    this.timings.set(stage, value);
  }

  admit(input: string, now = performance.now()): void {
    this.inputs.delete(input);
    this.inputs.set(input, { admitted: now, dequeued: false });
    const oldest = this.inputs.keys().next().value;
    if (this.inputs.size > 1000 && oldest !== undefined)
      this.inputs.delete(oldest);
  }

  forgetInput(input: string): void {
    this.inputs.delete(input);
  }

  dequeued(input: string, now = performance.now()): void {
    const timing = this.inputs.get(input);
    if (!timing || timing.dequeued) return;
    timing.dequeued = true;
    this.time("Queue (includes batching)", now - timing.admitted);
  }

  validated(durationMs: number, inputs: number, requests: number): void {
    this.time("Source validation", durationMs);
    this.validationInputs += inputs;
    this.validationRequests += requests;
  }

  committed(durationMs: number): void {
    this.time("Decision commit", durationMs);
  }

  planReply(parts: string[], inputs: string[], now = performance.now()): void {
    const tracked = new Map<string, InputTiming>();
    for (const input of inputs) {
      const timing = this.inputs.get(input);
      if (timing) {
        timing.planned = true;
        tracked.set(input, timing);
      }
    }
    const reply: ReplyTiming = {
      pending: new Set(parts),
      inputs: tracked,
      committed: now,
    };
    for (const part of parts) this.replies.set(part, reply);
    while (this.replies.size > 1000) {
      const oldest = this.replies.keys().next().value;
      if (oldest === undefined) break;
      this.replies.delete(oldest);
    }
  }

  deliveryStarted(part: string, now = performance.now()): void {
    const reply = this.replies.get(part);
    if (!reply || reply.started !== undefined) return;
    reply.started = now;
    this.time("Delivery queue", now - reply.committed);
  }

  delivered(part: string, now = performance.now()): void {
    const reply = this.replies.get(part);
    if (!reply) return;
    this.replies.delete(part);
    reply.pending.delete(part);
    if (reply.pending.size) return;
    if (reply.started !== undefined) this.time("Delivery", now - reply.started);
    for (const [input, timing] of reply.inputs) {
      // Edits/retractions replace or remove the admission being measured.
      if (this.inputs.get(input) !== timing) continue;
      this.time("End-to-end reply", now - timing.admitted);
      if (timing.calls !== undefined) {
        this.replyCallTotal += timing.calls;
        this.replyCallReports++;
      }
      this.inputs.delete(input);
    }
  }

  prepared(durationMs: number): void {
    this.preparations++;
    this.preparationMs += durationMs;
    this.time("Context preparation", durationMs);
  }

  record(call: ModelCall, inputs: string[] = []): void {
    for (const input of new Set(inputs)) {
      const timing = this.inputs.get(input);
      if (timing && !timing.planned) timing.calls = (timing.calls ?? 0) + 1;
    }
    this.attempts++;
    if (call.success) this.successes++;
    else this.failures++;
    this.durationMs += call.durationMs;
    this.time("Model attempt", call.durationMs);
    if (call.firstDeltaMs !== undefined) {
      this.firstDeltaMs += call.firstDeltaMs;
      this.firstDeltaReports++;
      this.time("First streamed delta", call.firstDeltaMs);
    }
    const usage = call.usage;
    this.latestInput = usage?.prompt;
    this.latestOutput = usage?.completion;
    this.latestBudget = call.inputBudget;
    if (usage?.prompt !== undefined) {
      this.prompt += usage.prompt;
      this.promptReports++;
    }
    if (usage?.completion !== undefined) {
      this.completion += usage.completion;
      this.completionReports++;
    }
    if (usage?.cached !== undefined) {
      this.cached += usage.cached;
      this.cacheReports++;
      if (usage.prompt !== undefined) {
        this.cachePrompt += usage.prompt;
        this.ratioReports++;
      }
    }
  }

  report(limits?: MetricLimits): string {
    const counter = (value: number, reports: number) =>
      reports
        ? `${value} (${reports}/${this.attempts} attempts reported)`
        : "unknown (provider did not report)";
    const ms = (value: number) =>
      value < 1000
        ? `${Math.round(value)} ms`
        : `${(value / 1000).toFixed(2)} s`;
    const stages: Stage[] = [
      "End-to-end reply",
      "Queue (includes batching)",
      "Source validation",
      "Context preparation",
      "Model attempt",
      "First streamed delta",
      "Decision commit",
      "Delivery queue",
      "Delivery",
    ];
    const rows = stages.map((stage) => {
      const value = this.timings.get(stage);
      if (!value) return `| ${stage} | unknown | unknown | unknown | 0 |`;
      const sorted = [...value.recent].sort((a, b) => a - b);
      const p95 = sorted[Math.ceil(sorted.length * 0.95) - 1];
      return `| ${stage} | ${ms(value.latest)} | ${ms(value.total / value.count)} | ${ms(p95)} | ${value.count} |`;
    });
    const details: [string, string][] = [
      [
        "Model attempts",
        `${this.attempts} (${this.successes} succeeded; ${this.failures} failed)`,
      ],
      [
        "Model calls per answered input",
        this.replyCallReports
          ? `${(this.replyCallTotal / this.replyCallReports).toFixed(2)} (${this.replyCallReports} inputs measured; shared batch calls count for each input)`
          : "unknown",
      ],
      ["Input tokens", counter(this.prompt, this.promptReports)],
      ["Output tokens", counter(this.completion, this.completionReports)],
      ["Cached input tokens", counter(this.cached, this.cacheReports)],
      [
        "Cache hit ratio",
        this.ratioReports === this.cacheReports && this.cachePrompt > 0
          ? `${((100 * this.cached) / this.cachePrompt).toFixed(1)}% (reported requests only)`
          : "unknown",
      ],
      [
        "Latest request input/output",
        `${this.latestInput ?? "unknown"} / ${this.latestOutput ?? "unknown"} tokens`,
      ],
      [
        "Pre-request input budget",
        this.latestBudget
          ? `${this.latestBudget.tokens} tokens (${this.latestBudget.source})`
          : "unknown",
      ],
      [
        "Source validation",
        `${this.validationInputs} inputs / ${this.validationRequests} HTTP reads`,
      ],
      [
        "History context",
        `${this.contextMessages} messages; ${this.contextChars} characters; ${this.rebases} rebuilds`,
      ],
    ];
    if (limits) {
      details.push([
        "Context limits",
        `compact at ${limits.compactTokens} tokens; total ${limits.contextTokens} tokens (including output reserve)`,
      ]);
      details.push([
        "Autonomous collection window",
        `${limits.batchWindowMs} ms`,
      ]);
    }
    return [
      "**Metrics for this conversation**",
      "",
      "Since worker start or metrics eviction. No model call is needed for this report.",
      "",
      "**Latency**",
      "",
      "| Stage | Latest | Mean | P95 | Samples |",
      "| --- | ---: | ---: | ---: | ---: |",
      ...rows,
      "",
      "End-to-end runs from admission to the final acknowledged reply part, including tool waits and retries. Local commands and silence are excluded. Unknown admission times after restart/eviction stay unknown. Stage sample counts differ and rows must not be added; first delta overlaps inference. P95 uses the last 128 samples.",
      "",
      "**Usage and context**",
      "",
      "| Metric | Value |",
      "| --- | --- |",
      ...details.map(([key, value]) => `| ${key} | ${value} |`),
      "",
      "Attempts include retries/tool rounds. Missing usage is unknown, not zero. Provider token counters are not billing estimates.",
    ].join("\n");
  }
}
