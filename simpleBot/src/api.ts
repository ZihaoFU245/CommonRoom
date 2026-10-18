// Typed HTTP client for the frozen-ish v1 bot surface.
//
// Design notes:
//  - `Idempotency-Key` is mandatory on sends; callers supply a key that is stable
//    across retries so a replayed request returns `replayed:true`.
//  - Only retryable failures are retried. A `409 conflict` is safe to retry for
//    history/reaction preflight but never for a send with fresh content.
//  - 429 `Retry-After` is honoured, and the server's own budgets are respected:
//    messages, reactions, reads, credential issuance and conversation creation
//    share 30 inputs per 10 s per account.
import type {
  ApiErrorBody,
  ConversationRequest,
  CredentialRequest,
  ErrorCode,
  HistoryQuery,
  HistoryResponse,
  MessageRequest,
  SendResponse,
  Snapshot,
} from "./wire.ts";
import { createHash } from "node:crypto";
import { setTimeout as wait } from "node:timers/promises";
import {
  isApiError,
  isServerCommands,
  isCommandResult,
  isMessageContext,
  isHistoryResponse,
  isSendResponse,
  isSnapshot,
} from "./wire.ts";

export class BotApiError extends Error {
  readonly status: number;
  readonly code: ErrorCode | "network";
  readonly retryable: boolean;
  readonly retryAfter: number;

  constructor(
    status: number,
    body:
      ApiErrorBody | { code: "network"; message: string; retryable: boolean },
    retryAfter = 0,
  ) {
    super(body.message);
    this.name = "BotApiError";
    this.status = status;
    this.code = body.code;
    this.retryable = body.retryable;
    this.retryAfter = retryAfter;
  }
}

export interface BotApiOptions {
  baseUrl: string;
  token: string;
  timeoutMs?: number;
  log?: (level: "debug" | "warn", message: string) => void;
}

export class BotApi {
  readonly base: URL;
  private readonly token: string;
  private readonly timeoutMs: number;
  private readonly log: (level: "debug" | "warn", message: string) => void;

  constructor(options: BotApiOptions) {
    const base = new URL(options.baseUrl);
    if (base.protocol !== "http:" && base.protocol !== "https:") {
      throw new Error("chat.url must use http or https");
    }
    base.pathname = `${base.pathname.replace(/\/+$/, "")}/api/bot/v1/`;
    base.search = "";
    base.hash = "";
    this.base = base;
    this.token = options.token;
    this.timeoutMs = options.timeoutMs ?? 20_000;
    this.log = options.log ?? (() => {});
  }

  /** Authenticated WebSocket URL and handshake headers. */
  socket(): { url: string; headers: Record<string, string> } {
    const url = new URL("ws", this.base);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    return { url: url.toString(), headers: this.headers() };
  }

  me(): Promise<Snapshot> {
    return this.request<Snapshot>("GET", "me");
  }

  history(query: HistoryQuery, signal?: AbortSignal): Promise<HistoryResponse> {
    const params = new URLSearchParams({
      conversation_id: query.conversation_id,
    });
    for (const key of [
      "start",
      "end",
      "before",
      "after",
      "around",
      "limit",
      "content_revision",
      "message_id",
    ] as const) {
      const value = query[key];
      if (value !== undefined) params.set(key, String(value));
    }
    return this.request<HistoryResponse>(
      "GET",
      `history?${params}`,
      undefined,
      { signal },
    );
  }

  /** Send text. `key` must be stable across retries of the same logical output. */
  send(input: MessageRequest, key: string): Promise<SendResponse> {
    return this.request<SendResponse>("POST", "messages", input, { key });
  }

  /**
   * Clear the shared unread badge. This is the human-facing read cursor for the
   * whole account, not a worker checkpoint, and it needs the `read_ack`
   * operation plus an explicit acknowledgement flag.
   */
  acknowledgeRead(
    conversationId: string,
    through: number,
    signal?: AbortSignal,
  ): Promise<{ ok: true }> {
    return this.request<{ ok: true }>(
      "POST",
      "read",
      {
        conversation_id: conversationId,
        through,
        acknowledge_shared_unread: true,
      },
      { signal },
    );
  }

  /** Desired-state reaction: repeating the same call is a no-op. */
  react(
    conversationId: string,
    messageId: string,
    reaction: string,
    active: boolean,
  ): Promise<{ ok: true }> {
    return this.request<{ ok: true }>("PUT", "reactions", {
      conversation_id: conversationId,
      message_id: messageId,
      reaction,
      active,
    });
  }

  createConversation(
    input: ConversationRequest,
  ): Promise<{ conversation_id: string }> {
    return this.request<{ conversation_id: string }>(
      "POST",
      "conversations",
      input,
    );
  }

  issueCredential(
    input: CredentialRequest,
  ): Promise<{ id: string; token: string; expires: number }> {
    // Issuance is not retried without an operator confirming its outcome.
    return this.request("POST", "credentials", input, { attempts: 0 });
  }

  schema(): Promise<Record<string, unknown>> {
    return this.request<Record<string, unknown>>("GET", "schema");
  }
  messageContext(
    conversationId: string,
    messageId: string,
  ): Promise<{
    author: {
      id: string;
      username: string;
      groups: string[];
      permissions: string[] | null;
    };
    bot: {
      id: string;
      groups: string[];
      owner_id: string | null;
      owner: string | null;
    };
  }> {
    return this.request(
      "GET",
      `message-context?${new URLSearchParams({ conversation_id: conversationId, message_id: messageId })}`,
    );
  }
  serverCommands(conversationId: string): Promise<{
    conversation_id: string;
    commands: import("./wire.ts").BotCommand[];
  }> {
    return this.request(
      "GET",
      `commands?${new URLSearchParams({ conversation_id: conversationId })}`,
    );
  }
  executeCommand(
    conversationId: string,
    command: string,
    key: string,
    signal?: AbortSignal,
  ): Promise<{ result: string; replayed: boolean; expires: number }> {
    // The persisted key binds retries to the same atomic server outcome.
    return this.request(
      "POST",
      "commands",
      { conversation_id: conversationId, command },
      { key, attempts: 2, signal },
    );
  }

  commandReceipt(key: string, signal?: AbortSignal): Promise<CommandReceipt> {
    return this.request(
      "GET",
      `commands/receipts/${encodeURIComponent(key)}`,
      undefined,
      { signal },
    );
  }

  previewSupported = false;
  async supportsActivity(): Promise<boolean> {
    const schema = await this.schema();
    const definitions = schema.definitions;
    this.previewSupported = false;
    if (typeof definitions !== "object" || definitions === null) return false;
    const activity = (definitions as Record<string, unknown>).ActivityRequest;
    if (typeof activity === "object" && activity !== null) {
      const properties = (activity as Record<string, unknown>).properties;
      this.previewSupported =
        typeof properties === "object" &&
        properties !== null &&
        "draft" in properties;
    }
    const frames = (definitions as Record<string, unknown>).ClientFrame;
    if (typeof frames !== "object" || frames === null) return false;
    const choices = (frames as Record<string, unknown>).oneOf;
    return (
      Array.isArray(choices) &&
      choices.some(
        (choice: unknown) =>
          typeof choice === "object" &&
          choice !== null &&
          (choice as Record<string, unknown>).$ref ===
            "#/definitions/ActivityRequest",
      )
    );
  }

  private headers(extra: Record<string, string> = {}): Record<string, string> {
    return { Authorization: `Bearer ${this.token}`, ...extra };
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    options: { key?: string; attempts?: number; signal?: AbortSignal } = {},
  ): Promise<T> {
    const attempts = options.attempts ?? 4;
    const idempotent =
      method === "GET" ||
      method === "PUT" ||
      path === "read" ||
      options.key !== undefined;
    for (let attempt = 0; ; attempt++) {
      options.signal?.throwIfAborted();
      try {
        return await this.once<T>(
          method,
          path,
          body,
          options.key,
          options.signal,
        );
      } catch (error) {
        options.signal?.throwIfAborted();
        const retryable = error instanceof BotApiError && error.retryable;
        const network =
          error instanceof BotApiError && error.code === "network";
        // A pinned history conflict needs a fresh snapshot, not the same stale
        // request. Creation cannot be retried after an ambiguous network failure.
        const conflict =
          error instanceof BotApiError &&
          error.code === "conflict" &&
          !(
            method === "GET" &&
            path.startsWith("history?") &&
            !new URLSearchParams(path.slice(path.indexOf("?") + 1)).has(
              "content_revision",
            )
          );
        if (
          attempt >= attempts ||
          !(retryable || network) ||
          !idempotent ||
          conflict
        )
          throw error;
        const waitMs = Math.max(
          error instanceof BotApiError ? error.retryAfter * 1000 : 0,
          Math.min(1000 * 2 ** attempt, 8000),
        );
        this.log(
          "debug",
          `${method} ${path} attempt ${attempt + 1} failed (${(error as BotApiError).code}); retrying in ${Math.round(waitMs / 1000)}s`,
        );
        await wait(waitMs, undefined, { signal: options.signal });
      }
    }
  }

  private async once<T>(
    method: string,
    path: string,
    body?: unknown,
    key?: string,
    signal?: AbortSignal,
  ): Promise<T> {
    const headers = this.headers(
      body === undefined ? {} : { "Content-Type": "application/json" },
    );
    if (key !== undefined) headers["Idempotency-Key"] = key;
    let response: Response;
    try {
      response = await fetch(new URL(path, this.base), {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.any([
          ...(signal ? [signal] : []),
          AbortSignal.timeout(this.timeoutMs),
        ]),
      });
    } catch (error) {
      const name = (error as Error)?.name;
      const detail =
        name === "TimeoutError"
          ? "Request timed out"
          : `Request failed: ${(error as Error).message}`;
      throw new BotApiError(0, {
        code: "network",
        message: detail,
        retryable: true,
      });
    }
    const text = await response.text();
    let value: unknown;
    try {
      value = text.length === 0 ? {} : JSON.parse(text);
    } catch {
      throw new BotApiError(response.status, {
        code: "network",
        message: "Server returned a non-JSON body",
        retryable: response.status >= 500,
      });
    }
    if (!response.ok) {
      const error =
        typeof value === "object" && value !== null
          ? (value as Record<string, unknown>).error
          : undefined;
      const retryAfter = Number(response.headers.get("retry-after")) || 0;
      throw new BotApiError(
        response.status,
        isApiError(error)
          ? error
          : {
              code: "network",
              message: `HTTP ${response.status}`,
              retryable: response.status >= 500,
            },
        retryAfter,
      );
    }
    if (response.headers.get("bot-api-version") !== "1") {
      this.log("warn", "response is missing the Bot-Api-Version: 1 header");
    }
    const endpoint = path.split("?")[0];
    if (
      (endpoint === "me" && !isSnapshot(value)) ||
      (endpoint === "credentials" &&
        !(
          typeof value === "object" &&
          value !== null &&
          "id" in value &&
          typeof value.id === "string" &&
          "token" in value &&
          typeof value.token === "string" &&
          /^bot_[a-f0-9]{64}$/.test(value.token) &&
          "expires" in value &&
          Number.isSafeInteger(value.expires)
        )) ||
      (endpoint === "message-context" && !isMessageContext(value)) ||
      (endpoint === "history" && !isHistoryResponse(value)) ||
      (endpoint === "messages" && !isSendResponse(value)) ||
      (path.startsWith("commands/receipts/") && !isCommandReceipt(value)) ||
      (endpoint === "commands" &&
        !(method === "GET" ? isServerCommands(value) : isCommandResult(value)))
    ) {
      throw new Error(`Invalid Bot API ${endpoint} response`);
    }
    return value as T;
  }
}

/** Stable per-output idempotency key: conversation, input message and slot. */
export function idempotencyKey(
  conversationId: string,
  messageId: string,
  slot: string,
): string {
  const raw = `${conversationId}:${messageId}:${slot}`;
  if (/^[A-Za-z0-9._:-]{1,128}$/.test(raw)) return raw;
  return `key-${createHash("sha256")
    .update(JSON.stringify([conversationId, messageId, slot]))
    .digest("hex")}`;
}

/** Split text into chunks the API accepts, preferring paragraph boundaries. */
export function chunkMessage(text: string, limit = 3900): string[] {
  const characters = Array.from(text);
  if (characters.length <= limit) return [text];
  const parts: string[] = [];
  let current = "";
  const flush = () => {
    if (current.length > 0) parts.push(current);
    current = "";
  };
  for (const paragraph of text.split(/(?<=\n)(?=\S)/)) {
    const candidate =
      current.length === 0 ? paragraph : `${current}${paragraph}`;
    if (Array.from(candidate).length <= limit) {
      current = candidate;
      continue;
    }
    flush();
    let rest = Array.from(paragraph);
    while (rest.length > limit) {
      parts.push(rest.slice(0, limit).join(""));
      rest = rest.slice(limit);
    }
    current = rest.join("");
  }
  flush();
  return parts.filter((part) => part.length > 0);
}

export type CommandReceipt =
  | { state: "not_found" }
  | {
      state: "succeeded" | "failed";
      result: string | null;
      error: ApiErrorBody | null;
      created: number;
      expires: number;
    };
function isCommandReceipt(value: unknown): value is CommandReceipt {
  if (typeof value !== "object" || value === null || !("state" in value))
    return false;
  if (value.state === "not_found") return true;
  return (
    (value.state === "succeeded" || value.state === "failed") &&
    "result" in value &&
    (value.result === null || typeof value.result === "string") &&
    "error" in value &&
    (value.error === null || isApiError(value.error)) &&
    "created" in value &&
    Number.isSafeInteger(value.created) &&
    "expires" in value &&
    Number.isSafeInteger(value.expires) &&
    (value.state === "succeeded"
      ? typeof value.result === "string" && value.error === null
      : value.result === null && isApiError(value.error))
  );
}
