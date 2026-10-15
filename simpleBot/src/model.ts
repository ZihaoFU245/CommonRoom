import { abortable } from "./async.ts";
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
  executeTool?(name: string, argumentsText: string): Promise<string>;
  maxTokens?: number;
  maxToolCalls?: number;
  /** A finishing tool has persisted the complete response plan. */
  stopAfterTool?(): boolean;
  /** Omit to use the provider default; support depends on the endpoint. */
  reasoningEffort?: string;
  temperature?: number;
  /** Called for each streamed delta when streaming is enabled. */
  onDelta?(text: string): void;
  signal?: AbortSignal;
  /** Numeric diagnostics only, once per model attempt (including tool rounds). */
  onCall?(call: ModelCall): void;
  contextTokens?: number;
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
}

export class ModelClient implements ChatModel {
  private readonly options: ModelOptions;
  private streamUsage = true;
  private readonly tokenMeters = new Map<string, ProviderTokens>();

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

  /** One completion, streamed or buffered. Retries transient gateway failures. */
  async complete(request: CompletionRequest): Promise<string> {
    const signal = AbortSignal.any([
      ...(request.signal ? [request.signal] : []),
      AbortSignal.timeout(this.options.timeoutMs),
    ]);
    return abortable(this.run({ ...request, signal }), signal);
  }

  private async run(request: CompletionRequest): Promise<string> {
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
    for (let round = 0; round <= limit; round++) {
      request.signal?.throwIfAborted();
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
      if (!turn.calls.length) return turn.text;
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
          content = await request.executeTool(
            call.function.name,
            call.function.arguments,
          );
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
      if (!response.ok)
        this.options.log(
          "error",
          `model upstream rejected request status=${response.status}`,
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
      await response.body?.cancel();
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
    // Some compatible gateways reject this optional standard field. Fall back
    // only on an explicit field rejection, never on a general provider failure.
    if (this.streamUsage && [400, 422].includes(response.status)) {
      const error = await response.text();
      if (/stream_options|include_usage/u.test(error)) {
        this.streamUsage = false;
        throw new ModelError(
          0,
          "gateway rejected optional stream usage; retrying without it",
        );
      }
    }
    if (!response.ok) {
      await response.body?.cancel();
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
          this.options.log(
            "debug",
            `model stream first delta kind=${kind} duration_ms=${Math.round(performance.now() - started)}`,
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
          if (
            !record(update) ||
            !Number.isInteger(update.index) ||
            typeof update.index !== "number" ||
            update.index < 0 ||
            update.index >= 16
          )
            throw new ModelError(200, "Invalid streamed tool index");
          const call = calls.get(update.index) ?? {
            id: "",
            type: "function",
            function: { name: "", arguments: "" },
          };
          if (typeof update.id === "string") call.id += update.id;
          if (typeof update.type === "string") call.type = update.type;
          if (record(update.function)) {
            if (typeof update.function.name === "string")
              call.function.name += update.function.name;
            if (typeof update.function.arguments === "string")
              call.function.arguments += update.function.arguments;
          }
          if (
            call.id.length > 128 ||
            call.function.name.length > 64 ||
            call.function.arguments.length > 65536
          )
            throw new ModelError(200, "Streamed tool call too large");
          calls.set(update.index, call);
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
    };
  }
}
