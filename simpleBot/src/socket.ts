// Bot WebSocket: notifications in, command discovery/activity frames out.
//
// The socket is a signal channel, not a log. On every `conversation` frame the
// agent refetches the snapshot and compares `content_revision`; a `Lagged`
// broadcast simply means "resync now". The server pings every 25 s and closes
// after 75 s without incoming traffic, so reconnects are expected and normal.
import type { ActivityState, ServerFrame } from "./wire.ts";
import { parseServerFrame } from "./wire.ts";

export interface SocketEvents {
  snapshot(frame: Extract<ServerFrame, { kind: "snapshot" }>): void;
  conversation(conversationId: string, contentRevision: number): void;
  presence(online: string[]): void;
  activity(frame: Extract<ServerFrame, { kind: "activity" }>): void;
  commandsRequest(
    frame: Extract<ServerFrame, { kind: "commands_request" }>,
  ): void;
  socketError(frame: Extract<ServerFrame, { kind: "error" }>): void;
  status(connected: boolean, detail: string): void;
  lagged(): void;
  sync(): void;
  /** Called once when entering standby, and once when it ends. */
  standby(waiting: boolean): void;
}

export interface BotSocketOptions {
  url: string;
  headers: Record<string, string>;
  events: SocketEvents;
  /** Older v1 servers only accept configuration frames. Check once per connection. */
  supportsActivity(): Promise<boolean>;
  supportsPreview?(): boolean;
  log(level: "debug" | "info" | "warn" | "error", message: string): void;
}

export class BotSocket {
  private readonly options: BotSocketOptions;
  private socket: WebSocket | null = null;
  private backoffMs = 1_000;
  private timer: NodeJS.Timeout | null = null;
  private closed = false;
  private generation = 0;
  /** True after a failed attempt, so standby reconnection is not logged repeatedly. */
  private standby = false;
  private activityEnabled = false;
  private activityScope: Set<string> | null = null;
  private activityAllowed = false;
  private commandsRegistered = false;

  constructor(options: BotSocketOptions) {
    this.options = options;
  }

  get connected(): boolean {
    return this.socket?.readyState === 1;
  }

  open(): void {
    this.closed = false;
    this.connect();
  }

  close(): void {
    this.closed = true;
    this.generation++;
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    const socket = this.socket;
    this.socket = null;
    if (socket !== null) {
      socket.onopen = null;
      socket.onclose = null;
      socket.onerror = null;
      socket.onmessage = null;
      try {
        socket.close();
      } catch {
        // Closing an already-failed socket is not an error worth reporting.
      }
    }
  }

  /** Transient presence signal. Silently dropped while disconnected. */
  activity(conversationId: string, state: ActivityState): void {
    if (
      !this.activityEnabled ||
      !this.activityAllowed ||
      (this.activityScope !== null && !this.activityScope.has(conversationId))
    )
      return;
    this.send({ kind: "activity", conversation_id: conversationId, state });
  }

  preview(conversationId: string, draft: string): void {
    // Provisional text yields to durable/control traffic on a slow connection.
    if ((this.socket?.bufferedAmount ?? 0) > 65536) return;
    if (
      !this.activityEnabled ||
      !this.activityAllowed ||
      !this.options.supportsPreview?.() ||
      (this.activityScope !== null && !this.activityScope.has(conversationId))
    )
      return;
    this.send({
      kind: "activity",
      conversation_id: conversationId,
      state: "working",
      draft,
    });
  }

  respondCommands(
    id: string,
    commands: import("./wire.ts").BotCommand[],
  ): void {
    this.send({ kind: "commands_response", id, commands });
  }
  commandsChanged(): void {
    this.send({ kind: "commands_changed" });
  }

  private send(frame: unknown): boolean {
    const socket = this.socket;
    if (socket === null || socket.readyState !== 1) return false;
    try {
      socket.send(JSON.stringify(frame));
      return true;
    } catch (error) {
      this.options.log(
        "warn",
        `socket send failed: ${(error as Error).message}`,
      );
      return false;
    }
  }

  private connect(): void {
    if (this.closed) return;
    this.activityEnabled = false;
    this.activityAllowed = false;
    this.commandsRegistered = false;
    const generation = ++this.generation;
    let socket: WebSocket;
    try {
      // Node's WebSocket forwards the `headers` option; verified against the
      // server's Authorization-based socket upgrade.
      socket = new WebSocket(this.options.url, {
        headers: this.options.headers,
      } as unknown as string[]);
    } catch (error) {
      this.options.log(
        "error",
        `socket construction failed: ${(error as Error).message}`,
      );
      this.retry();
      return;
    }
    this.socket = socket;

    socket.onopen = () => {
      if (generation !== this.generation || this.closed) return;
      this.backoffMs = 1_000;
      if (this.standby) {
        this.standby = false;
        this.options.events.standby(false);
      }
      this.options.events.status(true, "connected");
      void this.options
        .supportsActivity()
        .then((supported) => {
          if (generation !== this.generation || this.closed) return;
          this.activityEnabled = supported;
          if (!supported)
            this.options.log(
              "info",
              "server does not advertise activity support; activity signals disabled",
            );
        })
        .catch(() => {
          // Activity is advisory. A failed capability lookup must not stop chat.
          this.options.log(
            "debug",
            "activity capability unavailable; signals disabled until reconnect",
          );
        });
    };
    socket.onmessage = (event: MessageEvent) => {
      if (generation !== this.generation) return;
      this.dispatch(event.data);
    };
    socket.onerror = () => {
      // `onclose` always follows; the error itself is not actionable.
    };
    socket.onclose = (event: CloseEvent) => {
      if (generation !== this.generation) return;
      this.socket = null;
      // While standing by, only the first failure is worth reporting.
      if (!this.standby)
        this.options.events.status(
          false,
          event.reason || `closed (${event.code})`,
        );
      this.retry();
    };
  }

  /**
   * Standby: the server may be offline for a long time, so back off to one
   * attempt a minute and log the state change once instead of every attempt.
   */
  private retry(): void {
    if (this.closed || this.timer !== null) return;
    const wait = this.backoffMs;
    this.backoffMs = Math.min(this.backoffMs * 2, 30_000);
    if (!this.standby) {
      this.standby = true;
      this.options.events.standby(true);
    }
    this.timer = setTimeout(() => {
      this.timer = null;
      this.connect();
    }, wait);
  }

  private dispatch(raw: unknown): void {
    let value: unknown;
    try {
      value =
        typeof raw === "string" ? JSON.parse(raw) : JSON.parse(String(raw));
    } catch {
      this.options.log(
        "warn",
        "ignored a socket frame that was not valid JSON",
      );
      return;
    }
    const frame = parseServerFrame(value);
    if (frame === null) {
      this.options.log(
        "warn",
        "ignored a socket frame that did not match the v1 contract",
      );
      return;
    }
    switch (frame.kind) {
      case "snapshot":
        if (
          !this.commandsRegistered &&
          (frame.credential.operations === null ||
            frame.credential.operations.includes("read"))
        ) {
          this.commandsRegistered = this.send({ kind: "commands_ready" });
        }
        this.activityAllowed =
          frame.credential.operations === null ||
          frame.credential.operations.includes("activity");
        this.activityScope = new Set(
          frame.conversations
            .filter((c) => c.permissions.send)
            .map((c) => c.id),
        );
        this.options.events.snapshot(frame);
        return;
      case "conversation":
        this.options.events.conversation(
          frame.conversation_id,
          frame.content_revision,
        );
        return;
      case "presence":
        this.options.events.presence(frame.online);
        return;
      case "activity":
        this.options.events.activity(frame);
        return;
      case "commands_request":
        this.options.events.commandsRequest(frame);
        return;
      case "config_ack":
        this.options.log(
          "debug",
          `server acknowledged ${frame.id ?? "control"}`,
        );
        return;
      case "error":
        if (frame.error.retryable) {
          this.options.events.sync();
        } else {
          this.options.events.socketError(frame);
        }
        return;
      default:
        return;
    }
  }
}
