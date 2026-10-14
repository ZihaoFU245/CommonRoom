import { randomUUID } from "node:crypto";
import { readFile, rename, writeFile } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";
import WebSocket from "ws";
import { validate } from "./contract.mjs";

export class BotApiError extends Error {
  constructor(status, detail, retryAfter = 0) {
    super(detail.message);
    Object.assign(this, detail, { status, retryAfter });
  }
}
export class BotApi {
  constructor({ url, token, fetchImpl = fetch, sleep = delay }) {
    this.base = new URL(url);
    if (
      !["http:", "https:"].includes(this.base.protocol) ||
      this.base.search ||
      this.base.hash ||
      this.base.username ||
      this.base.password
    )
      throw new Error("Use an HTTP(S) server URL including its base path.");
    this.base.pathname = `${this.base.pathname.replace(/\/$/, "")}/api/bot/v1/`;
    Object.assign(this, { token, fetch: fetchImpl, sleep });
  }
  async request(
    path,
    {
      body,
      key,
      method = body === undefined
        ? "GET"
        : path === "reactions"
          ? "PUT"
          : "POST",
      signal,
    } = {},
  ) {
    const endpoint = path.split("?")[0];
    const requests = {
      messages: "MessageRequest",
      read: "ReadRequest",
      reactions: "ReactionRequest",
    };
    const responses = {
      me: "Snapshot",
      history: "HistoryResponse",
      messages: "SendResponse",
      read: "Ok",
      reactions: "Ok",
    };
    if (body !== undefined) validate(requests[endpoint], body);
    if (endpoint === "messages") validate("IdempotencyKey", key);
    const encoded = body === undefined ? undefined : JSON.stringify(body);
    const safe =
      method === "GET" ||
      endpoint === "messages" ||
      endpoint === "read" ||
      endpoint === "reactions";
    for (let attempt = 0; ; attempt++) {
      try {
        const response = await this.fetch(new URL(path, this.base), {
          method,
          headers: {
            Authorization: `Bearer ${this.token}`,
            ...(body ? { "Content-Type": "application/json" } : {}),
            ...(key ? { "Idempotency-Key": key } : {}),
          },
          ...(encoded === undefined ? {} : { body: encoded }),
          signal: signal
            ? AbortSignal.any([signal, AbortSignal.timeout(15000)])
            : AbortSignal.timeout(15000),
        });
        let value;
        try {
          value = await response.json();
        } catch {
          if (response.status >= 500)
            throw new BotApiError(response.status, {
              code: "unavailable",
              message: "Upstream unavailable.",
              retryable: true,
            });
          throw new Error("Invalid API response.");
        }
        if (!response.ok)
          throw new BotApiError(
            response.status,
            validate("ErrorResponse", value).error,
            Number(response.headers.get("retry-after")) || 0,
          );
        return responses[endpoint]
          ? validate(responses[endpoint], value)
          : value;
      } catch (error) {
        if (
          signal?.aborted ||
          !safe ||
          attempt >= 4 ||
          (error instanceof BotApiError
            ? !error.retryable || error.code === "idempotency_capacity"
            : !(error instanceof TypeError) && error.name !== "TimeoutError")
        )
          throw error;
        await this.sleep(
          Math.max(error.retryAfter || 0, Math.min(2 ** attempt, 16)) * 1000,
          undefined,
          { signal },
        );
      }
    }
  }
  socket() {
    const url = new URL("ws", this.base);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    return new WebSocket(url, {
      headers: { Authorization: `Bearer ${this.token}` },
      handshakeTimeout: 15000,
      maxPayload: 8 * 1024 * 1024,
    });
  }
}

// One process owns this file. Outbox completion and cursor advancement commit
// together; an interrupted handler reuses the saved payload and key.
export class AgentClient {
  constructor({
    api,
    statePath,
    onMessage,
    onGap = () => {},
    replayHistory = false,
  }) {
    Object.assign(this, { api, statePath, onMessage, onGap, replayHistory });
    this.state = {
      version: 1,
      account: null,
      cursors: Object.create(null),
      outbox: Object.create(null),
    };
    this.snapshot = null;
    this.active = null;
  }
  async load() {
    try {
      const state = JSON.parse(await readFile(this.statePath, "utf8"));
      const record = (v) => v && typeof v === "object" && !Array.isArray(v);
      if (
        state.version !== 1 ||
        (state.account !== null && typeof state.account !== "string") ||
        !record(state.cursors) ||
        !record(state.outbox) ||
        !Object.values(state.cursors).every(
          (n) => Number.isSafeInteger(n) && n >= 0,
        )
      )
        throw new Error("Invalid agent checkpoint.");
      for (const entry of Object.values(state.outbox)) {
        if (
          !record(entry) ||
          !Number.isFinite(entry.created) ||
          typeof entry.source !== "string" ||
          !Number.isSafeInteger(entry.sequence) ||
          entry.sequence < 1
        )
          throw new Error("Invalid agent outbox.");
        validate("MessageRequest", entry.input);
        validate("IdempotencyKey", entry.key);
        if (entry.response !== undefined)
          validate("SendResponse", entry.response);
      }
      state.cursors = Object.assign(Object.create(null), state.cursors);
      state.outbox = Object.assign(Object.create(null), state.outbox);
      this.state = state;
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  async save() {
    const temporary = `${this.statePath}.${randomUUID()}.tmp`;
    await writeFile(temporary, `${JSON.stringify(this.state)}\n`, {
      mode: 0o600,
    });
    await rename(temporary, this.statePath);
  }
  async send(text, { replyTo, markdown = true, slot = "answer" } = {}) {
    if (!this.active) throw new Error("Send inside onMessage.");
    const source = `${this.active.conversation}:${this.active.message.id}`;
    const operation = `${source}:${slot}`;
    let entry = this.state.outbox[operation];
    if (!entry) {
      if (Object.keys(this.state.outbox).length >= 4096)
        throw new Error("Agent outbox is full.");
      const input = validate("MessageRequest", {
        conversation_id: this.active.conversation,
        text,
        markdown,
        ...(replyTo ? { reply_to: replyTo } : {}),
      });
      entry = this.state.outbox[operation] = {
        key: randomUUID(),
        input,
        created: Date.now(),
        sequence: this.active.message.sequence,
        source,
      };
    }
    if (entry.response) return entry.response;
    if (Date.now() - entry.created >= 23 * 60 * 60 * 1000)
      throw new Error(
        "Pending send is near receipt expiry; inspect history before resolving it.",
      );
    await this.save();
    entry.response = await this.api.request("messages", {
      body: entry.input,
      key: entry.key,
      signal: this.signal,
    });
    await this.save();
    return entry.response;
  }
  async sync(snapshot) {
    this.snapshot = validate("Snapshot", snapshot);
    const account = snapshot.account.id;
    if (this.state.account && this.state.account !== account)
      throw new Error("Checkpoint belongs to another account.");
    this.state.account = account;
    const live = new Set(snapshot.conversations.map((c) => c.id));
    for (const entry of Object.values(this.state.outbox))
      if (!live.has(entry.input.conversation_id))
        throw new Error(
          "Pending send target is inaccessible; inspect the outbox.",
        );
    for (const { id, view, unread } of snapshot.conversations) {
      if (this.state.cursors[id] === undefined) {
        this.state.cursors[id] = this.replayHistory ? 0 : unread.through;
        await this.save();
      }
      if (this.state.cursors[id] > 0 && this.state.cursors[id] < unread.oldest)
        await this.onGap({
          view,
          conversation: id,
          after: this.state.cursors[id],
          oldest: unread.oldest,
        });
      for (;;) {
        const after = this.state.cursors[id];
        const page = validate(
          "HistoryResponse",
          await this.api.request(
            `history?${new URLSearchParams({ conversation_id: id, after: String(after), limit: "100" })}`,
            { signal: this.signal },
          ),
        );
        if (page.conversation_id !== id)
          throw new Error("History identity mismatch.");
        for (const message of page.messages) {
          for (const entry of Object.values(this.state.outbox))
            if (
              entry.input.conversation_id === id &&
              entry.sequence < message.sequence
            )
              throw new Error("Pending input was removed; inspect the outbox.");
          if (message.sequence <= this.state.cursors[id]) continue;
          this.active = { conversation: id, message };
          try {
            if (message.author_id !== account)
              await this.onMessage(message, {
                view,
                conversation: id,
                username: snapshot.account.username,
                client: this,
              });
          } finally {
            this.active = null;
          }
          this.state.cursors[id] = message.sequence;
          for (const [operation, entry] of Object.entries(this.state.outbox))
            if (entry.source === `${id}:${message.id}`)
              delete this.state.outbox[operation];
          await this.save();
        }
        if (!page.has_after) {
          if (
            Object.values(this.state.outbox).some(
              (entry) => entry.input.conversation_id === id,
            )
          )
            throw new Error("Pending input was removed; inspect the outbox.");
          break;
        }
        if (after === this.state.cursors[id])
          throw new Error("History pagination made no progress.");
      }
    }
    for (const id of Object.keys(this.state.cursors))
      if (!live.has(id)) delete this.state.cursors[id];
    await this.save();
  }
  async run({ signal } = {}) {
    this.signal = signal;
    await this.load();
    let backoff = 1000;
    while (!signal?.aborted) {
      let failure, socket;
      let dirty = false,
        running = false,
        work = Promise.resolve();
      const schedule = () => {
        dirty = true;
        if (running) return;
        running = true;
        work = (async () => {
          while (dirty && !signal?.aborted) {
            dirty = false;
            await this.sync(await this.api.request("me", { signal }));
          }
        })()
          .catch((error) => {
            failure = error;
            socket?.close();
          })
          .finally(() => {
            running = false;
          });
      };
      try {
        await this.sync(await this.api.request("me", { signal }));
        if (signal?.aborted) return;
        socket = this.api.socket();
        const abort = () => socket.close();
        signal?.addEventListener("abort", abort, { once: true });
        try {
          await new Promise((resolve) => {
            socket.on("message", (raw) => {
              try {
                const frame = validate(
                  "ServerFrame",
                  JSON.parse(raw.toString()),
                );
                if (frame.kind === "snapshot" || frame.kind === "conversation")
                  schedule();
              } catch (error) {
                failure = error;
                socket.close();
              }
            });
            socket.on("open", () => {
              backoff = 1000;
              if (signal?.aborted) abort();
            });
            socket.on("error", () => socket.close());
            socket.on("close", resolve);
          });
          await work;
        } finally {
          signal?.removeEventListener("abort", abort);
          socket.terminate();
        }
        if (failure) throw failure;
      } catch (error) {
        if (signal?.aborted) return;
        if (
          error instanceof BotApiError
            ? !error.retryable &&
              error.code !== "not_found" &&
              error.code !== "forbidden"
            : !(error instanceof TypeError) && error.name !== "TimeoutError"
        )
          throw error;
        // Reconcile after concurrent permission removal/deletion; writes with
        // lost permission preserve their pending outbox for operator inspection.
        if (
          error instanceof BotApiError &&
          ["not_found", "forbidden"].includes(error.code) &&
          Object.keys(this.state.outbox).length
        )
          throw error;
      }
      if (signal?.aborted) return;
      try {
        await this.api.sleep(backoff, undefined, { signal });
      } catch (error) {
        if (signal?.aborted) return;
        throw error;
      }
      backoff = Math.min(backoff * 2, 30000);
    }
  }
}
