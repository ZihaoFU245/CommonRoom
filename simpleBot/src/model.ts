import { finishTickTool, type TickDecision } from "./tick.ts";
import { tickResponse } from "./tick-response.ts";
import { tickResponseShape } from "./model-diagnostics.ts";
import { abortable } from "./async.ts";
import { ToolJobs, WAIT_JOBS_TOOL } from "./tool-jobs.ts";
import { ToolLocks, type ToolAccess } from "./tool-locks.ts";
import { setTimeout as wait } from "node:timers/promises";
// Model client for an OpenAI-compatible chat-completions endpoint.
//
// Nothing is preset: an operator supplies `model.endpoint` and `model.name` at
// runtime. Keep prompts and responses out of logs unless you opt in; chat
// content is private.
import { estimateTokens, ProviderTokens, type TokenCount } from "./tokens.ts";

export interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool";
  tool_call_id?: string;
  tool_calls?: ToolCall[];
  reasoning_content?: string;
  content: string;
}

export interface ToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}
export interface FunctionTool {
  type: "function";
  function: {
    name: string;
    description: string;
    parameters: Record<string, unknown>;
  };
}
interface Turn {
  text: string;
  calls: ToolCall[];
  reasoning?: string;
  usage?: ModelUsage;
  firstDeltaMs?: number;
}

export interface ModelUsage {
  prompt?: number;
  completion?: number;
  cached?: number;
}
export interface ModelCall {
  success: boolean;
  durationMs: number;
  usage?: ModelUsage;
  inputBytes?: number;
  inputBudget?: TokenCount;
  /** Includes response headers and the first text, reasoning or tool delta. */
  firstDeltaMs?: number;
}

/** Optional provider telemetry is untrusted; absent/invalid counters stay unknown. */
function modelUsage(value: unknown): ModelUsage | undefined {
  if (!record(value)) return undefined;
  const count = (n: unknown): number | undefined =>
    typeof n === "number" && Number.isSafeInteger(n) && n >= 0 ? n : undefined;
  const prompt = count(value.prompt_tokens);
  const completion = count(value.completion_tokens);
  const details = record(value.prompt_tokens_details)
    ? value.prompt_tokens_details
    : {};
  let cached =
    count(value.prompt_cache_hit_tokens) ?? count(details.cached_tokens);
  if (cached !== undefined && prompt !== undefined && cached > prompt)
    cached = undefined;
  return {
    ...(prompt === undefined ? {} : { prompt }),
    ...(completion === undefined ? {} : { completion }),
    ...(cached === undefined ? {} : { cached }),
  };
}
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function toolCalls(value: unknown): ToolCall[] {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value) || value.length > 16)
    throw new ModelError(200, "Invalid model tool calls");
  return value.map((call: unknown) => {
    if (
      !record(call) ||
      typeof call.id !== "string" ||
      !call.id ||
      call.id.length > 128 ||
      call.type !== "function" ||
      !record(call.function) ||
      typeof call.function.name !== "string" ||
      typeof call.function.arguments !== "string" ||
      call.function.arguments.length > 65536
    )
      throw new ModelError(200, "Invalid model tool call");
    return {
      id: call.id,
      type: "function",
      function: {
        name: call.function.name,
        arguments: call.function.arguments,
      },
    };
  });
}

export interface ModelOptions {
  endpoint: string;
  token: string;
  model: string;
  timeoutMs: number;
  stream: boolean;
  log(level: "debug" | "info" | "warn" | "error", message: string): void;
}

export class ModelError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ModelError";
    this.status = status;
  }
}

export interface CompletionRequest {
  messages: ChatMessage[];
  tools?: FunctionTool[];
  /** Host-approved message IDs for targeted replies in this tick. */
  replyTargets?: string[];
  maxReplyMessages?: number;
  /** One bounded recovery tick with no tools or continuation after repeated invalid plans. */
  answerOnly?: boolean;
  /** Diagnostics only; never serialized into the provider payload. */
  tickId?: number;
  executeTool?(
    name: string,
    argumentsText: string,
    signal?: AbortSignal,
    effectId?: string,
    onYield?: () => Promise<void>,
  ): Promise<string>;
  /** Return job handles for slow tools and resume inference in the meantime. */
  backgroundTools?: boolean;
  /** Trusted resource policy; model arguments cannot grant read-only access. */
  toolAccess?(name: string, argumentsText: string): ToolAccess;
  maxTokens?: number;
  maxToolCalls?: number;
  /** A finishing tool has persisted the complete response plan. */
  stopAfterTool?(): boolean;
  /** Omit to use the provider default; support depends on the endpoint. */
  reasoningEffort?: string;
  temperature?: number;
  /** Streamed deltas are provisional until complete() accepts a final answer. */
  onDelta?(text: string): void;
  signal?: AbortSignal;
  /** Numeric diagnostics only, once per model attempt (including tool rounds). */
  onCall?(call: ModelCall): void;
  contextTokens?: number;
  /** Remaining episode allowance, including this request input and output. */
  episodeTokens?: number;
  compactTokens?: number;
  /** Scope measurements to a conversation; omit for a standalone completion. */
  contextKey?: string;
  resetTokenBaseline?: boolean;
  compact?(
    messages: ChatMessage[],
    target: number,
    tokensPerByte: number,
  ): ChatMessage[];
}

/** Resolve the chat-completions URL from a gateway base or explicit path. */
export function completionsUrl(endpoint: string): string {
  const url = new URL(endpoint);
  const path = url.pathname.replace(/\/+$/, "");
  if (path.endsWith("/chat/completions")) return url.toString();
  url.pathname = path.endsWith("/v1")
    ? `${path}/chat/completions`
    : `${path}/v1/chat/completions`;
  return url.toString();
}

/** The surface the agent depends on, so tests can substitute a stub. */
export interface ChatModel {
  readonly describe: string;
  complete(request: CompletionRequest): Promise<string>;
  tick?(request: CompletionRequest): Promise<TickDecision>;
}

export class ModelClient implements ChatModel {
  private readonly options: ModelOptions;
  private streamUsage = true;
  private readonly tokenMeters = new Map<string, ProviderTokens>();
  private readonly toolLocks = new ToolLocks();

  constructor(options: ModelOptions) {
    this.options = options;
  }

  get describe(): string {
    if (!this.configured) return "not configured";
    return `${this.options.model} at ${new URL(this.options.endpoint).host}`;
  }

  /** True once an endpoint and a model id have been configured. */
  get configured(): boolean {
    return this.options.endpoint.length > 0 && this.options.model.length > 0;
  }

  /** Exactly one provider request. Retries and tools belong to the event loop. */
  async tick(request: CompletionRequest): Promise<TickDecision> {
    const tools = (request.answerOnly ? [] : (request.tools ?? [])).filter(
      (tool) => !["finish_turn", "wait_jobs"].includes(tool.function.name),
    );
    const envelope = finishTickTool(tools);
    const advertised = request.answerOnly ? [] : [envelope];
    const meter = request.contextKey
      ? (this.tokenMeters.get(request.contextKey) ?? new ProviderTokens())
      : new ProviderTokens();
    if (request.resetTokenBaseline) meter.reset();
    if (request.contextKey) {
      this.tokenMeters.delete(request.contextKey);
      this.tokenMeters.set(request.contextKey, meter);
      if (this.tokenMeters.size > 16)
        this.tokenMeters.delete(this.tokenMeters.keys().next().value!);
    }
    let messages = [...request.messages];
    const total = Math.min(
      request.contextTokens ?? 32768,
      request.episodeTokens ?? Infinity,
    );
    const target = Math.min(
      request.compactTokens ?? 24576,
      total - Math.min(request.maxTokens ?? 4096, Math.floor(total / 4)),
    );
    let budget = meter.count(messages, advertised, request.reasoningEffort);
    if (budget.tokens >= target && request.compact) {
      messages = request.compact(
        messages,
        Math.max(
          128,
          target - estimateTokens({ tools: advertised }, meter.tokensPerByte),
        ),
        meter.tokensPerByte,
      );
      meter.reset();
      budget = meter.count(messages, advertised, request.reasoningEffort);
    }
    const maxTokens = Math.min(
      request.maxTokens ?? 4096,
      total - budget.tokens,
    );
    if (maxTokens < 128)
      throw new ModelError(400, "model context budget exhausted");
    const signal = request.signal
      ? AbortSignal.any([
          request.signal,
          AbortSignal.timeout(this.options.timeoutMs),
        ])
      : AbortSignal.timeout(this.options.timeoutMs);
    const started = performance.now();
    const trace = `conversation=${request.contextKey ?? "standalone"} tick=${request.tickId ?? "standalone"}`;
    this.options.log(
      "debug",
      `model tick request ${trace} model=${JSON.stringify(this.options.model)} stream=${this.options.stream} messages=${messages.length} advertised_tools=${advertised.length} native_tools=${tools.length} input_budget=${budget.tokens} budget_source=${budget.source} max_tokens=${maxTokens} reasoning_effort=${request.reasoningEffort || "provider_default"} reply_targets=${request.replyTargets?.length ?? "unspecified"} max_reply_messages=${request.maxReplyMessages ?? "unspecified"}`,
    );
    try {
      const input = {
        ...request,
        messages,
        tools: advertised,
        maxTokens,
        signal,
      };
      const turn = await abortable(
        this.options.stream ? this.stream(input) : this.buffered(input),
        signal,
      );
      this.options.log(
        "debug",
        `model tick response ${trace} ${tickResponseShape(
          turn.text,
          turn.calls,
          (request.tools ?? []).map((tool) => tool.function.name),
        )}`,
      );
      request.onCall?.({
        success: true,
        durationMs: performance.now() - started,
        inputBytes: Buffer.byteLength(
          JSON.stringify({ messages, tools: advertised }),
        ),
        inputBudget: budget,
        ...(turn.firstDeltaMs === undefined
          ? {}
          : { firstDeltaMs: turn.firstDeltaMs }),
        ...(turn.usage ? { usage: turn.usage } : {}),
      });
      if (turn.usage?.prompt !== undefined)
        meter.observe(
          messages,
          advertised,
          request.reasoningEffort,
          turn.usage.prompt,
        );
      // Auto tool selection permits an ordinary assistant answer. Preserve the
      // pre-tick behavior instead of asking the model to answer a second time.
      if (turn.calls.length === 0) {
        this.options.log(
          "debug",
          `model tick accepted ${trace} format=final_text messages=${turn.text ? 1 : 0} actions=0 next=wait`,
        );
        return {
          messages: turn.text ? [{ text: turn.text }] : [],
          actions: [],
          next: { kind: "wait" },
        };
      }
      const available = new Set([
        "finish_tick",
        ...(request.answerOnly
          ? []
          : (request.tools ?? []).map((tool) => tool.function.name)),
      ]);
      if (
        turn.calls.filter((call) => call.function.name === "finish_tick")
          .length > 1 ||
        turn.calls.some((call) => !available.has(call.function.name))
      ) {
        const names = turn.calls
          .map((call) =>
            available.has(call.function.name)
              ? call.function.name
              : "unavailable",
          )
          .join(",");
        this.options.log(
          "warn",
          `model tick rejected ${trace} reason=unexpected_tool_calls count=${turn.calls.length} tools=${names}`,
        );
        throw new ModelError(
          422,
          `Expected at most one finish_tick decision and available native tools (received ${turn.calls.length}: ${names})`,
        );
      }
      try {
        const { decision, coalesced, normalized } = tickResponse(
          turn.calls,
          tools,
        );
        const format = turn.calls.every(
          (call) => call.function.name === "finish_tick",
        )
          ? "finish_tick"
          : "native_tools";
        this.options.log(
          "debug",
          `model tick accepted ${trace} format=${format} messages=${decision.messages.length} actions=${decision.actions.length} next=${decision.next.kind} coalesced_actions=${coalesced} normalized=${normalized.length}`,
        );
        if (normalized.length)
          // Labels are host-owned tool and field names, never model input.
          this.options.log(
            "warn",
            `model tick arguments normalized ${trace} count=${normalized.length} labels=${JSON.stringify([...new Set(normalized)])}`,
          );
        return decision;
      } catch (error) {
        this.options.log(
          "warn",
          `model tick rejected ${trace} reason=invalid_decision validation=${error instanceof Error ? error.message : "invalid_arguments"}`,
        );
        throw new ModelError(
          422,
          `Invalid finish_tick decision: ${error instanceof Error ? error.message : "invalid arguments"}`,
        );
      }
    } catch (error) {
      // Validation failures follow a successful inference; do not double-count it.
      if (!(error instanceof ModelError && error.status === 422))
        request.onCall?.({
          success: false,
          durationMs: performance.now() - started,
          inputBudget: budget,
        });
      throw error;
    }
  }

  /** One completion, streamed or buffered. Retries transient gateway failures. */
  async complete(request: CompletionRequest): Promise<string> {
    const controller = new AbortController();
    const signal = AbortSignal.any([
      controller.signal,
      ...(request.signal ? [request.signal] : []),
      AbortSignal.timeout(this.options.timeoutMs),
    ]);
    const jobs =
      request.backgroundTools && request.executeTool && request.tools?.length
        ? new ToolJobs(
            request.executeTool,
            request.toolAccess ??
              (() => ({ resource: "tools", mode: "write" })),
            this.toolLocks,
            signal,
          )
        : undefined;
    try {
      return await abortable(
        this.run(
          {
            ...request,
            signal,
            ...(jobs
              ? { tools: [...(request.tools ?? []), WAIT_JOBS_TOOL] }
              : {}),
          },
          jobs,
        ),
        signal,
      );
    } finally {
      controller.abort();
      await jobs?.close();
    }
  }

  private async run(
    request: CompletionRequest,
    jobs?: ToolJobs,
  ): Promise<string> {
    if (this.options.endpoint.length === 0) {
      throw new ModelError(0, "no model.endpoint is configured");
    }
    if (this.options.model.length === 0) {
      throw new ModelError(0, "no model.name is configured");
    }
    let messages = [...request.messages];
    const meter = request.contextKey
      ? (this.tokenMeters.get(request.contextKey) ?? new ProviderTokens())
      : new ProviderTokens();
    if (request.resetTokenBaseline) meter.reset();
    if (request.contextKey) {
      this.tokenMeters.delete(request.contextKey);
      this.tokenMeters.set(request.contextKey, meter);
      if (this.tokenMeters.size > 16)
        this.tokenMeters.delete(this.tokenMeters.keys().next().value!);
    }
    let executed = 0;
    const limit = Math.max(1, Math.min(request.maxToolCalls ?? 32, 128));
    // A premature final answer may require one extra step per background call.
    for (let round = 0; round <= (jobs ? limit * 2 + 1 : limit); round++) {
      request.signal?.throwIfAborted();
      const completed = jobs?.collect() ?? [];
      if (completed.length)
        messages.push({
          role: "user",
          content: JSON.stringify({
            type: "tool_job_results",
            jobs: completed,
          }),
        });
      const totalLimit = request.contextTokens ?? 32768;
      const reserve = Math.min(
        request.maxTokens ?? 4096,
        Math.floor(totalLimit / 4),
      );
      const target = Math.min(
        request.compactTokens ?? 24576,
        totalLimit - reserve,
      );
      let budget = meter.count(
        messages,
        request.tools,
        request.reasoningEffort,
      );
      if (budget.tokens >= target && request.compact) {
        this.options.log(
          "debug",
          `model compaction started round=${round + 1} input_budget=${budget.tokens} budget_source=${budget.source} target=${target}`,
        );
        const beforeCompaction = JSON.stringify(messages);
        const toolTokens = estimateTokens(
          { tools: request.tools },
          meter.tokensPerByte,
        );
        messages = request.compact(
          messages,
          Math.max(128, target - toolTokens - 64),
          meter.tokensPerByte,
        );
        if (JSON.stringify(messages) !== beforeCompaction) meter.reset();
        budget = meter.count(messages, request.tools, request.reasoningEffort);
        this.options.log(
          "debug",
          `model compaction finished round=${round + 1} input_budget=${budget.tokens} budget_source=${budget.source}`,
        );
      }
      const outputLimit = Math.min(
        request.maxTokens ?? 4096,
        totalLimit - budget.tokens,
      );
      if (outputLimit < 128)
        throw new ModelError(
          400,
          "model context budget exhausted; reduce input or configured prompt",
        );
      const turn = await this.turn(
        {
          ...request,
          messages,
          maxTokens: outputLimit,
        },
        budget,
      );
      if (turn.usage?.prompt !== undefined)
        meter.observe(
          messages,
          request.tools,
          request.reasoningEffort,
          turn.usage.prompt,
        );
      if (!turn.calls.length) {
        if (jobs?.outstanding) {
          // This draft did not see the results. Wait without polling, then let
          // the model produce the actual final answer using those results.
          messages.push({
            role: "user",
            content: JSON.stringify({
              type: "tool_job_results",
              jobs: await jobs.waitResults(),
            }),
          });
          continue;
        }
        return turn.text;
      }
      if (
        !request.tools?.length ||
        !request.executeTool ||
        executed + turn.calls.length > limit
      )
        throw new ModelError(200, "Model tool call limit or unavailable tools");
      messages.push({
        role: "assistant",
        content: turn.text,
        tool_calls: turn.calls,
        ...(turn.reasoning ? { reasoning_content: turn.reasoning } : {}),
      });
      let resultsAdded = false;
      for (const call of turn.calls) {
        request.signal?.throwIfAborted();
        executed++;
        const tool =
          request.tools.find(
            (tool) => tool.function.name === call.function.name,
          )?.function.name ?? "unknown";
        this.options.log(
          "debug",
          `model tool started round=${round + 1} tool_index=${executed} tool=${tool}`,
        );
        const started = performance.now();
        let content: string;
        try {
          if (jobs && call.function.name === "wait_jobs") {
            try {
              content = await jobs.wait(call.function.arguments);
            } catch (error) {
              request.signal?.throwIfAborted();
              content = JSON.stringify({
                error:
                  error instanceof Error ? error.message : "Invalid job IDs",
              });
            }
          } else if (jobs && call.function.name === "finish_turn") {
            content =
              jobs.outstanding || resultsAdded
                ? JSON.stringify({
                    error:
                      "Finish in a later model step after reviewing all tool results; use wait_jobs for outstanding jobs",
                  })
                : await request.executeTool(
                    call.function.name,
                    call.function.arguments,
                    request.signal,
                  );
          } else if (jobs) {
            content = await jobs.dispatch(
              call.function.name,
              call.function.arguments,
            );
          } else {
            content = await request.executeTool(
              call.function.name,
              call.function.arguments,
              request.signal,
            );
          }
        } catch (error) {
          this.options.log(
            "error",
            `model tool failed round=${round + 1} tool_index=${executed} tool=${tool} duration_ms=${Math.round(performance.now() - started)}`,
          );
          throw error;
        }
        this.options.log(
          "debug",
          `model tool finished round=${round + 1} tool_index=${executed} tool=${tool} result_chars=${content.length} duration_ms=${Math.round(performance.now() - started)}`,
        );
        messages.push({ role: "tool", tool_call_id: call.id, content });
        resultsAdded = true;
        if (request.stopAfterTool?.()) return "";
      }
    }
    throw new ModelError(200, "Model tool call limit reached");
  }
  private async turn(
    request: CompletionRequest,
    inputBudget: TokenCount,
  ): Promise<Turn> {
    let lastError: Error = new ModelError(0, "model call failed");
    for (let attempt = 0; attempt < 3; attempt++) {
      const started = performance.now();
      this.options.log(
        "debug",
        `model request started model=${JSON.stringify(this.options.model)} host=${new URL(this.options.endpoint).host} attempt=${attempt + 1} stream=${this.options.stream} messages=${request.messages.length} tools=${request.tools?.length ?? 0} input_budget=${inputBudget.tokens} budget_source=${inputBudget.source} max_tokens=${request.maxTokens ?? "provider_default"} timeout_ms=${this.options.timeoutMs}`,
      );
      try {
        const turn = this.options.stream
          ? await this.stream(request)
          : await this.buffered(request);
        const inputBytes = Buffer.byteLength(
          JSON.stringify({ messages: request.messages, tools: request.tools }),
          "utf8",
        );
        request.onCall?.({
          success: true,
          durationMs: performance.now() - started,
          inputBytes,
          inputBudget,
          ...(turn.firstDeltaMs === undefined
            ? {}
            : { firstDeltaMs: turn.firstDeltaMs }),
          ...(turn.usage ? { usage: turn.usage } : {}),
        });
        this.options.log(
          "debug",
          `model request finished attempt=${attempt + 1} duration_ms=${Math.round(performance.now() - started)} output_chars=${turn.text.length} tool_calls=${turn.calls.length} input_tokens=${turn.usage?.prompt ?? "unknown"} output_tokens=${turn.usage?.completion ?? "unknown"} cached_tokens=${turn.usage?.cached ?? "unknown"}`,
        );
        return turn;
      } catch (error) {
        request.onCall?.({
          success: false,
          inputBudget,
          durationMs: performance.now() - started,
        });
        lastError =
          error instanceof Error ? error : new Error("model call failed");
        const status = error instanceof ModelError ? error.status : 0;
        this.options.log(
          "error",
          `model request failed attempt=${attempt + 1} status=${status} duration_ms=${Math.round(performance.now() - started)} reason=${error instanceof ModelError ? error.message : "model transport failure"}`,
        );
        if (
          !(
            status === 0 ||
            status === 408 ||
            status === 429 ||
            status >= 500
          ) ||
          attempt === 2 ||
          request.signal?.aborted
        )
          break;
        const waitMs = 500 * 2 ** attempt;
        this.options.log(
          "warn",
          `model call failed (${lastError.message}); retrying in ${waitMs}ms`,
        );
        await wait(waitMs, undefined, { signal: request.signal });
      }
    }
    throw lastError;
  }

  private body(request: CompletionRequest, stream: boolean): string {
    return JSON.stringify({
      model: this.options.model,
      stream,
      ...(stream && this.streamUsage
        ? { stream_options: { include_usage: true } }
        : {}),
      messages: request.messages,
      // Leave tool selection at the compatible default. Some thinking models
      // reject a forced named tool; tick() enforces one finish_tick locally.
      ...(request.tools?.length ? { tools: request.tools } : {}),
      ...(request.maxTokens === undefined
        ? {}
        : { max_tokens: request.maxTokens }),
      ...(request.temperature === undefined
        ? {}
        : { temperature: request.temperature }),
      ...(request.reasoningEffort
        ? { reasoning_effort: request.reasoningEffort }
        : {}),
    });
  }

  /**
   * Read a bounded error body without letting a stalled gateway stall the
   * worker: the read races a short deadline and the stream is always cancelled.
   * Gateway failures are otherwise indistinguishable in a deployment log.
   */
  private async errorText(
    response: Response,
    limit = 4096,
    graceMs = 250,
  ): Promise<string> {
    const reader = response.body?.getReader();
    if (!reader) return "";
    const decoder = new TextDecoder();
    const deadline = AbortSignal.timeout(graceMs);
    let text = "";
    try {
      for (;;) {
        const { done, value } = await Promise.race([
          reader.read(),
          new Promise<never>((_resolve, reject) =>
            deadline.addEventListener(
              "abort",
              () => reject(new Error("error body deadline")),
              { once: true },
            ),
          ),
        ]);
        if (done) break;
        text += decoder.decode(value, { stream: true });
        if (text.length >= limit) break;
      }
    } catch {
      // A stalled or failed read still reports its status and size below.
    }
    await reader.cancel().catch(() => {});
    return text.slice(0, limit);
  }

  /**
   * Log why a gateway rejected a request without logging its body: providers
   * routinely quote prompt content and credentials in `error.message`. Only
   * allowlisted diagnostic labels and a size are safe here. Structural fields
   * are still provider-controlled and may contain private text or log newlines.
   */
  private logUpstream(status: number, body: string): void {
    let type = "";
    let code = "";
    let param = "";
    try {
      const value: unknown = JSON.parse(body);
      const error =
        typeof value === "object" && value !== null && "error" in value
          ? (value as { error: unknown }).error
          : undefined;
      const detail =
        typeof error === "object" && error !== null
          ? (error as Record<string, unknown>)
          : {};
      const safeLabel = (field: unknown, allowed: string[]): string =>
        typeof field === "string" && allowed.includes(field) ? field : "";
      type = safeLabel(detail.type, [
        "invalid_request_error",
        "authentication_error",
        "permission_error",
        "not_found_error",
        "rate_limit_error",
        "api_error",
        "server_error",
        "overloaded_error",
      ]);
      code = safeLabel(detail.code, [
        "bad_param",
        "invalid_api_key",
        "model_not_found",
        "context_length_exceeded",
        "insufficient_quota",
        "rate_limit_exceeded",
        "invalid_parameter",
        "unsupported_parameter",
        "invalid_value",
        "missing_required_parameter",
        "content_filter",
      ]);
      param = safeLabel(
        typeof detail.param === "string"
          ? detail.param.replace(/\[\d+\]/gu, "[]")
          : undefined,
        [
          "model",
          "messages",
          "messages[].role",
          "messages[].content",
          "messages[].tool_calls",
          "messages[].tool_call_id",
          "tools",
          "tools[].function.name",
          "tools[].function.description",
          "tools[].function.parameters",
          "tool_choice",
          "parallel_tool_calls",
          "max_tokens",
          "reasoning_effort",
          "stream",
          "stream_options",
          "stream_options.include_usage",
        ],
      );
    } catch {
      // A non-JSON body has no structural fields to report.
    }
    this.options.log(
      "error",
      `model upstream rejected request status=${status} body_chars=${body.length}` +
        `${type ? ` error_type=${type}` : ""}` +
        `${code ? ` error_code=${code}` : ""}` +
        `${param ? ` error_param=${param}` : ""}`,
    );
  }

  private headers(): Record<string, string> {
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
      Accept: "application/json",
    };
    if (this.options.token.length > 0)
      headers.Authorization = `Bearer ${this.options.token}`;
    return headers;
  }

  private async call(
    request: CompletionRequest,
    stream: boolean,
  ): Promise<Response> {
    const signal = request.signal
      ? AbortSignal.any([
          request.signal,
          AbortSignal.timeout(this.options.timeoutMs),
        ])
      : AbortSignal.timeout(this.options.timeoutMs);
    try {
      const response = await fetch(completionsUrl(this.options.endpoint), {
        method: "POST",
        headers: this.headers(),
        body: this.body(request, stream),
        signal,
      });
      this.options.log(
        "debug",
        `model HTTP response status=${response.status} stream=${stream}`,
      );
      return response;
    } catch (error) {
      const name = (error as Error).name;
      if (name === "AbortError") throw new ModelError(0, "model call aborted");
      if (name === "TimeoutError")
        throw new ModelError(408, "model call timed out");
      throw new ModelError(0, "model gateway unreachable");
    }
  }

  private async buffered(request: CompletionRequest): Promise<Turn> {
    const response = await this.call(request, false);
    if (!response.ok) {
      const body = await this.errorText(response);
      this.logUpstream(response.status, body);
      throw new ModelError(
        response.status,
        `model gateway returned HTTP ${response.status}`,
      );
    }
    const text = await response.text();
    let value: unknown;
    try {
      value = JSON.parse(text);
    } catch {
      throw new ModelError(
        response.status,
        "model gateway returned a non-JSON response",
      );
    }
    if (
      !record(value) ||
      !Array.isArray(value.choices) ||
      !record(value.choices[0])
    )
      throw new ModelError(200, "Invalid model response");
    const choice = value.choices[0];
    if (choice.finish_reason === "content_filter")
      throw new ModelError(
        403,
        "model provider blocked response (content_filter)",
      );
    if (choice.finish_reason === "length") throw this.tokenLimit(request);
    const message = choice.message;
    if (!record(message))
      throw new ModelError(200, "model response had no text content");
    if (typeof message.refusal === "string" && message.refusal.trim())
      throw new ModelError(403, "model provider refused response");
    const calls = toolCalls(message.tool_calls);
    const content =
      typeof message.content === "string" ? message.content.trim() : "";
    if (
      !content &&
      !calls.length &&
      !(
        choice.finish_reason === "stop" &&
        (message.content === null || typeof message.content === "string")
      )
    )
      throw new ModelError(200, "model response had no text content");
    const usage = modelUsage(value.usage);
    return {
      text: content,
      calls,
      ...(usage ? { usage } : {}),
      ...(typeof message.reasoning_content === "string"
        ? { reasoning: message.reasoning_content }
        : {}),
    };
  }

  private tokenLimit(request: CompletionRequest): ModelError {
    // Repeating the same budget cannot fix reasoning consuming all output tokens.
    return new ModelError(
      200,
      `model reached its output token limit (max_tokens=${request.maxTokens ?? "provider default"}); increase model.maxTokens or set model.reasoningEffort to none if supported`,
    );
  }

  private async stream(request: CompletionRequest): Promise<Turn> {
    const started = performance.now();
    const response = await this.call(request, true);
    if (!response.ok) {
      const body = await this.errorText(response);
      this.logUpstream(response.status, body);
      // Some compatible gateways reject this optional standard field. Fall back
      // only on an explicit field rejection, never on a general provider failure.
      if (
        this.streamUsage &&
        [400, 422].includes(response.status) &&
        /stream_options|include_usage/u.test(body)
      ) {
        this.streamUsage = false;
        throw new ModelError(
          0,
          "gateway rejected optional stream usage; retrying without it",
        );
      }
      throw new ModelError(
        response.status,
        `model gateway returned HTTP ${response.status}`,
      );
    }
    if (response.body === null)
      throw new ModelError(response.status, "streaming response had no body");
    const decoder = new TextDecoder();
    let buffer = "";
    let assembled = "";
    let done = false;
    let finishReason: unknown;
    let reasoning = "";
    let receivedDelta = false;
    let firstDeltaMs: number | undefined;
    let usage: ModelUsage | undefined;
    const calls = new Map<
      number,
      {
        id: string;
        type: string;
        function: { name: string; arguments: string };
      }
    >();
    const consume = (line: string): void => {
      if (!line.startsWith("data:")) return;
      const payload = line.slice(5).trim();
      if (payload.length === 0) return;
      if (payload === "[DONE]") {
        done = true;
        return;
      }
      let parsed: unknown;
      try {
        parsed = JSON.parse(payload);
      } catch {
        throw new ModelError(200, "model stream contained invalid JSON");
      }
      if (typeof parsed !== "object" || parsed === null)
        throw new ModelError(200, "invalid model stream event");
      const event = parsed as {
        error?: unknown;
        choices?: { delta?: { content?: unknown }; finish_reason?: unknown }[];
      };
      if (event.error !== undefined)
        throw new ModelError(200, "model stream reported an error");
      if (record(parsed) && parsed.usage !== undefined)
        usage = modelUsage(parsed.usage) ?? usage;
      const rawChoice =
        record(parsed) && Array.isArray(parsed.choices)
          ? parsed.choices[0]
          : null;
      const rawDelta =
        record(rawChoice) && record(rawChoice.delta) ? rawChoice.delta : null;
      if (record(rawChoice) && rawChoice.finish_reason === "content_filter")
        throw new ModelError(
          403,
          "model provider blocked response (content_filter)",
        );
      if (typeof rawDelta?.refusal === "string" && rawDelta.refusal.trim())
        throw new ModelError(403, "model provider refused response");
      if (!receivedDelta && rawDelta) {
        const kind =
          typeof rawDelta.content === "string" && rawDelta.content.length
            ? "text"
            : typeof rawDelta.reasoning_content === "string" &&
                rawDelta.reasoning_content.length
              ? "reasoning"
              : Array.isArray(rawDelta.tool_calls) && rawDelta.tool_calls.length
                ? "tool_call"
                : null;
        if (kind) {
          receivedDelta = true;
          firstDeltaMs = performance.now() - started;
          this.options.log(
            "debug",
            `model stream first delta kind=${kind} duration_ms=${Math.round(firstDeltaMs)}`,
          );
        }
      }
      if (rawDelta && typeof rawDelta.reasoning_content === "string")
        reasoning += rawDelta.reasoning_content;
      if (rawDelta?.tool_calls !== undefined) {
        if (
          !Array.isArray(rawDelta.tool_calls) ||
          rawDelta.tool_calls.length > 16
        )
          throw new ModelError(200, "Invalid streamed tool calls");
        for (const update of rawDelta.tool_calls as unknown[]) {
          if (!record(update))
            throw new ModelError(200, "Invalid streamed tool call");
          // Some compatible gateways omit the index after the first chunk of a
          // call. Attribute such a delta to the one call in progress instead of
          // discarding an otherwise valid response.
          let index: number;
          if (
            Number.isInteger(update.index) &&
            typeof update.index === "number"
          )
            index = update.index;
          else if (calls.size === 0) index = 0;
          else if (calls.size === 1)
            index = calls.keys().next().value as number;
          else throw new ModelError(200, "Invalid streamed tool index");
          if (index < 0 || index >= 16)
            throw new ModelError(200, "Invalid streamed tool index");
          const call = calls.get(index) ?? {
            id: "",
            type: "function",
            function: { name: "", arguments: "" },
          };
          // Repeat-tolerant accumulation: a gateway that resends the same id or
          // name in every chunk must not corrupt the call into an unavailable
          // tool. Genuinely split fragments still concatenate.
          if (typeof update.id === "string" && update.id.length) {
            if (!call.id) call.id = update.id;
            else if (call.id !== update.id) call.id += update.id;
          }
          if (typeof update.type === "string") call.type = update.type;
          if (record(update.function)) {
            const name = update.function.name;
            if (typeof name === "string" && name.length) {
              if (!call.function.name) call.function.name = name;
              else if (call.function.name !== name) call.function.name += name;
            }
            if (typeof update.function.arguments === "string")
              call.function.arguments += update.function.arguments;
          }
          if (
            call.id.length > 128 ||
            call.function.name.length > 64 ||
            call.function.arguments.length > 65536
          )
            throw new ModelError(200, "Streamed tool call too large");
          calls.set(index, call);
        }
      }
      const delta = event.choices?.[0]?.delta?.content;
      if (
        event.choices?.[0]?.finish_reason !== null &&
        event.choices?.[0]?.finish_reason !== undefined
      ) {
        finishReason = event.choices[0].finish_reason;
      }
      if (typeof delta === "string") {
        assembled += delta;
        request.onDelta?.(delta);
      }
    };
    for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) {
      buffer += decoder.decode(chunk, { stream: true });
      let boundary = buffer.indexOf("\n");
      while (boundary >= 0) {
        consume(buffer.slice(0, boundary).trim());
        buffer = buffer.slice(boundary + 1);
        if (done) break;
        boundary = buffer.indexOf("\n");
      }
      if (done) break;
    }
    if (!done) consume((buffer + decoder.decode()).trim());
    if (!done) throw new ModelError(0, "model stream ended before completion");
    if (finishReason === "length") throw this.tokenLimit(request);
    if (
      assembled.trim().length === 0 &&
      calls.size === 0 &&
      finishReason !== "stop"
    )
      throw new ModelError(0, "model stream produced no text");
    return {
      text: assembled.trim(),
      ...(usage ? { usage } : {}),
      calls: toolCalls(
        [...calls.entries()].sort(([a], [b]) => a - b).map(([, call]) => call),
      ),
      ...(reasoning ? { reasoning } : {}),
      ...(firstDeltaMs === undefined ? {} : { firstDeltaMs }),
    };
  }
}
