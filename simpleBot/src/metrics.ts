import type { ModelCall } from "./model.ts";
import type { TokenCount } from "./tokens.ts";

/** In-memory, per-conversation diagnostics. Never retains prompts or responses. */
export class Metrics {
  attempts = 0;
  successes = 0;
  failures = 0;
  durationMs = 0;
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

  record(call: ModelCall): void {
    this.attempts++;
    if (call.success) this.successes++;
    else this.failures++;
    this.durationMs += call.durationMs;
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

  report(): string {
    const counter = (value: number, reports: number) =>
      reports
        ? `${value} (${reports}/${this.attempts} attempts reported)`
        : "unknown (provider did not report)";
    return [
      "**Metrics for this conversation, since worker start or metrics eviction**",
      `Model attempts: ${this.attempts}; succeeded: ${this.successes}; failed: ${this.failures} (includes tool rounds and retries)`,
      `Mean model attempt latency: ${this.attempts ? `${Math.round(this.durationMs / this.attempts)} ms` : "unknown"}`,
      `Input tokens: ${counter(this.prompt, this.promptReports)}`,
      `Output tokens: ${counter(this.completion, this.completionReports)}`,
      `Latest request input/output (provider): ${this.latestInput ?? "unknown"} / ${this.latestOutput ?? "unknown"} tokens`,
      `Pre-request input budget: ${this.latestBudget ? `${this.latestBudget.tokens} tokens (${this.latestBudget.source})` : "unknown"}`,
      `Cached input tokens: ${counter(this.cached, this.cacheReports)}`,
      `Cache hit ratio: ${this.ratioReports === this.cacheReports && this.cachePrompt > 0 ? `${((100 * this.cached) / this.cachePrompt).toFixed(1)}% (reported requests only)` : "unknown"}`,
      `History context: ${this.contextMessages} messages; ${this.contextChars} characters; ${this.rebases} rebuilds`,
      "Missing usage is unknown, not zero. These are provider counters, not billing estimates.",
    ].join("\n");
  }
}
