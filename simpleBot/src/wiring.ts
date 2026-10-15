// Socket event wiring, shared by the entrypoint and the tests so the live
// configuration path is exercised by the same code in both.
import type { Agent } from "./agent.ts";
import type { ConfigStore } from "./config.ts";
import { isConfigKey } from "./config.ts";
import type { Logger } from "./log.ts";
import type { Store } from "./store.ts";
import type { SocketEvents } from "./socket.ts";
import type { ServerFrame } from "./wire.ts";

/** The only socket capability the wiring needs. */
export interface SocketSink {
  respondCommands(id: string, commands: import("./wire.ts").BotCommand[]): void;
  commandsChanged(): void;
  respond(
    id: string,
    values: Record<string, string> | null,
    error: string | null,
  ): void;
}

export interface WiringContext {
  config: ConfigStore;
  store: Store;
  log: Logger;
  /** Current agent, or null before the first build. */
  agent(): Agent | null;
  /** Current socket, for answering configuration requests. */
  socket(): SocketSink | null;
  /** Called after a configuration change that affects the transport. */
  onTransportChange(): void;
}

/** Build the socket event handlers. */
export function createSocketEvents(context: WiringContext): SocketEvents {
  const { config, store, log } = context;
  const answer = (
    id: string,
    values: Record<string, string> | null,
    error: string | null,
  ): void => {
    context.socket()?.respond(id, values, error);
  };

  return {
    commandsRequest(frame) {
      const agent = context.agent();
      if (!agent) {
        context.socket()?.respondCommands(frame.id, []);
        return;
      }
      void agent.commands(frame.conversation_id).then(
        (commands) => context.socket()?.respondCommands(frame.id, commands),
        () => context.socket()?.respondCommands(frame.id, []),
      );
    },
    status(connected, detail) {
      log(
        connected ? "info" : "warn",
        `socket ${connected ? "connected" : `disconnected (${detail})`}`,
      );
    },
    async snapshot(frame: Extract<ServerFrame, { kind: "snapshot" }>) {
      store.metrics.frames++;
      try {
        await context.agent()?.handleSnapshot(frame);
      } catch (error) {
        log("error", `snapshot handling failed: ${(error as Error).message}`);
      }
    },
    conversation(conversationId, revision) {
      store.metrics.frames++;
      log("debug", `notification ${conversationId} at revision ${revision}`);
      void context
        .agent()
        ?.requestSync()
        .catch((error) =>
          log("warn", `sync failed: ${(error as Error).message}`),
        );
    },
    presence: () => {
      // Handled by the caller: it owns the cached online directory.
    },
    activity: () => {
      // Other accounts' activity signals are advisory; this worker ignores them.
    },
    configRequest(frame) {
      if (frame.operation === "set") {
        if (frame.key === null || frame.value === null) {
          answer(frame.id, null, "set requires both key and value");
          return;
        }
        if (!isConfigKey(frame.key)) {
          answer(frame.id, null, `unsupported key ${frame.key}`);
          return;
        }
        const before = {
          url: config.get("chat.url"),
          token: config.get("chat.token"),
        };
        try {
          const values = config.apply(frame.key, frame.value);
          log("info", `config ${frame.key} updated by operator`);
          answer(frame.id, values, null);
          context.socket()?.commandsChanged();
        } catch (error) {
          answer(frame.id, null, `rejected: ${(error as Error).message}`);
          return;
        }
        if (
          config.get("chat.url") !== before.url ||
          config.get("chat.token") !== before.token
        ) {
          log("info", "transport settings changed; reconnecting");
          context.onTransportChange();
        }
        return;
      }
      try {
        const diagnostics = context.agent()?.operatorValues() ?? {};
        const values =
          frame.key === null
            ? { ...config.fetch(null), ...diagnostics }
            : frame.key in diagnostics
              ? { [frame.key]: diagnostics[frame.key] ?? "" }
              : config.fetch(frame.key);
        answer(frame.id, values, null);
      } catch (error) {
        answer(frame.id, null, `rejected: ${(error as Error).message}`);
      }
    },
    socketError(frame) {
      log(
        "error",
        `server rejected a frame: ${frame.error.code} ${frame.error.message}`,
      );
    },
    lagged() {
      log("warn", "socket fell behind; resyncing");
    },
    standby(waiting) {
      // One line per state change keeps a long outage quiet.
      log(
        "info",
        waiting
          ? "server offline: standing by, retrying quietly"
          : "server reachable again",
      );
    },
    sync() {
      void context
        .agent()
        ?.requestSync()
        .catch((error) =>
          log("warn", `sync failed: ${(error as Error).message}`),
        );
    },
  };
}
