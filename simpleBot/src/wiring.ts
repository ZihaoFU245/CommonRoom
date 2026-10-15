// Socket event wiring, shared by the entrypoint and the tests so the live
// command discovery path is exercised by the same code in both.
import type { Agent } from "./agent.ts";
import type { Logger } from "./log.ts";
import type { Store } from "./store.ts";
import type { SocketEvents } from "./socket.ts";
import type { ServerFrame } from "./wire.ts";

/** The only socket capability the wiring needs. */
export interface SocketSink {
  respondCommands(id: string, commands: import("./wire.ts").BotCommand[]): void;
  commandsChanged(): void;
}

export interface WiringContext {
  store: Store;
  log: Logger;
  /** Current agent, or null before the first build. */
  agent(): Agent | null;
  /** Current socket, for answering command catalogue requests. */
  socket(): SocketSink | null;
}

/** Build the socket event handlers. */
export function createSocketEvents(context: WiringContext): SocketEvents {
  const { store, log } = context;
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
