// A self-contained, in-process stand-in for the CommonRoom Bot API v1.
//
// It implements the same wire contract as the server (snapshot, history paging,
// revision-pinned scans, keyed idempotent sends, desired-state reactions, read
// acknowledgement, credential context, socket notifications, configuration
// requests) so the worker can be exercised without a running server or model.
import {
  createServer,
  type IncomingMessage,
  type Server,
  type ServerResponse,
} from "node:http";
import type { ClientFrame, Message, Snapshot } from "../src/wire.ts";

export interface MockOptions {
  token?: string;
  username?: string;
  accountId?: string;
  /** Restrict the credential, as `POST /credentials` would. */
  operations?: string[] | null;
  conversations?: string[] | null;
}

export interface SendRecord {
  key: string;
  text: string;
  replyTo: string | null;
  replayed: boolean;
  sequence: number;
}

export class MockServer {
  unreadUnavailable = false;
  operator = true;
  ownerId = "acct-owner";
  readonly commandReceipts = new Map<
    string,
    { command: string; result: string; created: number }
  >();
  readonly state = {
    revision: 7,
    messages: [] as Message[],
    sends: [] as SendRecord[],
    reactions: [] as {
      message_id: string;
      reaction: string;
      active: boolean;
    }[],
    reads: [] as { through: number }[],
    activities: [] as { conversation_id: string; state: string }[],

    historyCalls: 0,
    commands: [] as string[],
  };
  readonly conversationId = "conv-1";
  readonly view = "support";
  readonly token: string;
  readonly account: { id: string; username: string };
  private readonly operations: string[] | null;
  private readonly scopes: string[] | null;
  private server: Server | null = null;
  private sequence = 100;
  /** How many conversation notifications the server emitted. */
  notifications = 0;
  /** Frames the worker sent over its socket, in order. */
  readonly frames: ClientFrame[] = [];

  constructor(options: MockOptions = {}) {
    this.token = options.token ?? "bot_test";
    this.account = {
      id: options.accountId ?? "acct-bot",
      username: options.username ?? "helper",
    };
    this.operations = options.operations ?? null;
    this.scopes = options.conversations ?? null;
  }

  async listen(): Promise<string> {
    this.server = createServer((request, response) =>
      this.handle(request, response),
    );
    await new Promise<void>((resolve) =>
      this.server!.listen(0, "127.0.0.1", resolve),
    );
    const address = this.server.address();
    if (address === null || typeof address === "string")
      throw new Error("mock server has no port");
    return `http://127.0.0.1:${address.port}/`;
  }

  async close(): Promise<void> {
    await new Promise<void>((resolve) => this.server?.close(() => resolve()));
  }

  /** Add a message as another account would, bumping the revision. */
  addMessage(
    text: string,
    options: { from?: string; authorId?: string; mentions?: string[] } = {},
  ): Message {
    this.sequence += 1;
    const message: Message = {
      id: `m${this.sequence}`,
      from: options.from ?? "alice",
      author_id: options.authorId ?? "acct-alice",
      text,
      markdown: false,
      edited: false,
      time: Math.floor(Date.now() / 1000),
      sequence: this.sequence,
      reply: null,
      mentions: options.mentions ?? [this.account.username],
      reactions: {},
    };
    this.state.messages.push(message);
    this.state.revision += 1;
    this.notify();
    return message;
  }

  /** Simulate `/edit` by the original author: same id and sequence, new text. */
  editMessage(messageId: string, text: string): void {
    const message = this.state.messages.find(
      (candidate) => candidate.id === messageId,
    );
    if (message === undefined) throw new Error(`unknown message ${messageId}`);
    message.text = text;
    message.edited = true;
    this.state.revision += 1;
    this.notify();
  }

  /** Simulate `/retract`: the message leaves retained history. */
  retractMessage(messageId: string): void {
    this.state.messages = this.state.messages.filter(
      (message) => message.id !== messageId,
    );
    this.state.revision += 1;
    this.notify();
  }

  snapshot(): Snapshot {
    const incoming = this.state.messages.filter(
      (message) => message.author_id !== this.account.id,
    );
    const latest = this.state.messages.at(-1);
    const oldest = this.state.messages[0];
    const conversation = {
      id: this.conversationId,
      view: this.view,
      permissions: { send: true, reply: true, react: true },
      members: [this.account.username, "alice"],
      unread: {
        count: incoming.length,
        first: incoming[0]?.sequence ?? null,
        through: latest?.sequence ?? 0,
        oldest: oldest?.sequence ?? 0,
        revision: this.state.revision,
      },
      content_revision: this.state.revision,
    };
    return {
      kind: "snapshot",
      account: this.account,
      conversations: [
        {
          ...conversation,
          unread: this.unreadUnavailable ? null : conversation.unread,
        },
      ],
      credential: {
        id: "cred-test",
        expires: null,
        conversations: this.scopes,
        operations: this.operations,
      },
    };
  }

  /** Record a conversation notification the way the socket would deliver it. */
  private notify(): void {
    this.notifications += 1;
  }

  /**
   * Record a client frame. The test harness feeds these into the socket wiring
   * handler directly, so no real WebSocket connection is required.
   */
  receive(frame: ClientFrame): void {
    this.frames.push(frame);
    if (frame.kind === "activity") {
      this.state.activities.push({
        conversation_id: frame.conversation_id,
        state: frame.state,
      });
    }
  }

  private allowed(operation: string): boolean {
    return this.operations === null || this.operations.includes(operation);
  }

  private handle(request: IncomingMessage, response: ServerResponse): void {
    const url = new URL(request.url ?? "/", "http://localhost");
    const path = url.pathname.replace(/^\/api\/bot\/v1/, "");
    if (request.headers.authorization !== `Bearer ${this.token}`) {
      this.fail(response, 401, "unauthorized", "Bot credential required.");
      return;
    }
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => {
      const raw = Buffer.concat(chunks).toString();
      let body: Record<string, unknown> = {};
      try {
        body =
          raw.length === 0 ? {} : (JSON.parse(raw) as Record<string, unknown>);
      } catch {
        this.fail(response, 400, "invalid_request", "Invalid JSON.");
        return;
      }
      if (path.startsWith("/commands/receipts/")) {
        const receipt = this.commandReceipts.get(
          decodeURIComponent(path.slice("/commands/receipts/".length)),
        );
        return this.ok(
          response,
          receipt
            ? {
                state: "succeeded",
                result: receipt.result,
                error: null,
                created: receipt.created,
                expires: receipt.created + 86400,
              }
            : { state: "not_found" },
        );
      }
      switch (path) {
        case "/message-context": {
          if (!this.allowed("config")) {
            this.fail(response, 403, "forbidden", "Credential scope denied.");
            return;
          }
          const message = this.state.messages.find(
            (message) => message.id === url.searchParams.get("message_id"),
          );
          if (!message) {
            this.fail(response, 404, "not_found", "Source message not found.");
            return;
          }
          return this.ok(response, {
            author: {
              id: message.author_id,
              username: message.from,
              groups: [this.operator ? "admin" : "user"],
              permissions: null,
            },
            bot: {
              id: this.account.id,
              groups: [],
              owner_id: this.ownerId,
              owner: "owner",
            },
          });
        }
        case "/commands":
          if (!this.allowed("commands")) {
            this.fail(response, 403, "forbidden", "Credential scope denied.");
            return;
          }
          if (request.method === "GET")
            return this.ok(response, {
              conversation_id: this.conversationId,
              commands: [
                {
                  name: "/whoami",
                  usage: "/whoami",
                  description: "Show account identity",
                },
                {
                  name: "/new",
                  usage: "/new room name",
                  description: "Create a room when granted",
                },
                {
                  name: "/history",
                  usage: "/history [start end]",
                  description: "Read retained history",
                },
              ],
            });
          {
            const key = request.headers["idempotency-key"];
            if (typeof key !== "string")
              return this.fail(
                response,
                400,
                "invalid_request",
                "Key required",
              );
            const previous = this.commandReceipts.get(key);
            if (previous && previous.command !== body.command)
              return this.fail(
                response,
                409,
                "idempotency_conflict",
                "Different command",
              );
            const created = previous?.created ?? Math.floor(Date.now() / 1000);
            if (!previous) {
              this.state.commands.push(String(body.command));
              this.commandReceipts.set(key, {
                command: String(body.command),
                result: "Server command completed",
                created,
              });
            }
            return this.ok(response, {
              result: "Server command completed",
              replayed: !!previous,
              expires: created + 86400,
            });
          }
        case "/me":
          return this.ok(response, this.snapshot());
        case "/history":
          return this.history(response, url);
        case "/messages":
          return this.messages(response, request, body);
        case "/reactions":
          return this.reactions(response, body);
        case "/read":
          return this.read(Number(body.through ?? 0), response);
        case "/schema":
          return this.ok(response, {
            $id: "urn:commonroom:bot-api:v1",
            definitions: { Snapshot: {}, Message: {} },
          });
        case "/conversations":
          return this.ok(response, {
            conversation_id: `created-${String(body.kind)}`,
          });
        default:
          return this.fail(response, 404, "not_found", `no route ${path}`);
      }
    });
  }

  private history(response: ServerResponse, url: URL): void {
    this.state.historyCalls++;
    const pinned = url.searchParams.get("content_revision");
    if (pinned !== null && Number(pinned) !== this.state.revision) {
      this.fail(
        response,
        409,
        "conflict",
        "Content revision changed; rescan history.",
      );
      return;
    }
    const after = Number(url.searchParams.get("after") ?? 0);
    const before = url.searchParams.get("before");
    const referenced = url.searchParams.get("message_id");
    const anchorMessage = referenced
      ? this.state.messages.find((message) => message.id === referenced)
      : undefined;
    if (referenced && !anchorMessage) {
      this.fail(response, 404, "not_found", "Message not retained");
      return;
    }
    const around = anchorMessage
      ? String(anchorMessage.sequence)
      : url.searchParams.get("around");
    const start = url.searchParams.get("start");
    const end = url.searchParams.get("end");
    const limit = Math.min(Number(url.searchParams.get("limit") ?? 100), 100);
    let list = [...this.state.messages];
    if (around !== null) {
      const anchor = Number(around);
      list = list
        .filter((message) => message.sequence <= anchor)
        .slice(-Math.ceil(limit / 2))
        .concat(
          list
            .filter((message) => message.sequence > anchor)
            .slice(0, Math.ceil(limit / 2)),
        );
    } else {
      if (start !== null)
        list = list.filter((message) => message.sequence >= Number(start));
      if (end !== null)
        list = list.filter((message) => message.sequence <= Number(end));
      if (after > 0) list = list.filter((message) => message.sequence > after);
      if (before !== null)
        list = list.filter((message) => message.sequence < Number(before));
      if (before !== null) list = list.slice(-limit);
      else if (url.searchParams.has("after") || start !== null)
        list = list.slice(0, limit);
      else list = list.slice(-limit);
    }
    this.ok(response, {
      conversation_id: this.conversationId,
      messages: list,
      content_revision: this.state.revision,
      has_before:
        list.length > 0 &&
        list[0]!.sequence > (this.state.messages[0]?.sequence ?? 0),
      has_after:
        list.length > 0 &&
        list.at(-1)!.sequence < (this.state.messages.at(-1)?.sequence ?? 0),
    });
  }

  private messages(
    response: ServerResponse,
    request: IncomingMessage,
    body: Record<string, unknown>,
  ): void {
    if (!this.allowed("send")) {
      this.fail(response, 403, "forbidden", "Credential scope denied.");
      return;
    }
    const key = request.headers["idempotency-key"];
    if (typeof key !== "string" || key.length === 0) {
      this.fail(
        response,
        400,
        "invalid_request",
        "Idempotency-Key is required.",
      );
      return;
    }
    const previous = this.state.sends.find((record) => record.key === key);
    if (previous !== undefined) {
      const message = this.state.messages.find(
        (candidate) => candidate.sequence === previous.sequence,
      )!;
      this.ok(response, { message, replayed: true });
      return;
    }
    const text = String(body.text ?? "");
    if (text.length === 0 || Array.from(text).length > 4000) {
      this.fail(
        response,
        400,
        "invalid_request",
        "Messages support 1-4000 characters.",
      );
      return;
    }
    this.sequence += 1;
    const created: Message = {
      id: `m${this.sequence}`,
      from: this.account.username,
      author_id: this.account.id,
      text,
      markdown: body.markdown !== false,
      edited: false,
      time: Math.floor(Date.now() / 1000),
      sequence: this.sequence,
      reply: null,
      mentions: [],
      reactions: {},
    };
    this.state.messages.push(created);
    this.state.sends.push({
      key,
      text,
      replyTo: typeof body.reply_to === "string" ? body.reply_to : null,
      replayed: false,
      sequence: created.sequence,
    });
    this.state.revision += 1;
    this.ok(response, { message: created, replayed: false });
  }

  private reactions(
    response: ServerResponse,
    body: Record<string, unknown>,
  ): void {
    if (!this.allowed("react")) {
      this.fail(response, 403, "forbidden", "Credential scope denied.");
      return;
    }
    this.state.reactions.push({
      message_id: String(body.message_id),
      reaction: String(body.reaction),
      active: body.active === true,
    });
    this.state.revision += 1;
    this.ok(response, { ok: true });
  }

  private read(through: number, response: ServerResponse): void {
    if (!this.allowed("read_ack")) {
      this.fail(response, 403, "forbidden", "Credential scope denied.");
      return;
    }
    this.state.reads.push({ through });
    this.ok(response, { ok: true });
  }

  private ok(response: ServerResponse, body: unknown): void {
    const text = JSON.stringify(body);
    response.writeHead(200, {
      "Content-Type": "application/json",
      "Bot-Api-Version": "1",
    });
    response.end(text);
  }

  private fail(
    response: ServerResponse,
    status: number,
    code: string,
    message: string,
  ): void {
    response.writeHead(status, {
      "Content-Type": "application/json",
      "Bot-Api-Version": "1",
    });
    response.end(
      JSON.stringify({
        error: { code, message, retryable: code === "conflict" },
      }),
    );
  }
}
