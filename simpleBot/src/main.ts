// Entry point. Zero dependencies: Node 22.18+ runs this file directly via
// native TypeScript type stripping, and Node's built-in WebSocket sends the
// Authorization header that the bot socket handshake requires.
//
//   node simpleBot/src/main.ts
//
// Environment:
//   BOT_TOKEN   bot credential (overrides the persisted `chat.token`)
//   CHAT_URL    base URL (overrides the persisted `chat.url`)
//   BOT_CONFIG  config file  (default simpleBot/data/config.json)
//   BOT_STATE   checkpoint   (default simpleBot/data/checkpoint.json)
//   CHAT_DATA   directory for both defaults, when set
import { existsSync } from "node:fs";
import { join } from "node:path";
import { ConfigStore, redact, type ConfigKey } from "./config.ts";
import { BotApi, BotApiError } from "./api.ts";
import { BotSocket } from "./socket.ts";
import { ModelClient } from "./model.ts";
import { Store } from "./store.ts";
import { Agent } from "./agent.ts";
import { createSocketEvents } from "./wiring.ts";
import { LEVELS, type LogLevel } from "./log.ts";

const SESSION = "simpleBot";

function main(): void {
  const dataDir =
    process.env.CHAT_DATA ?? join(import.meta.dirname, "..", "data");
  const configPath = process.env.BOT_CONFIG ?? join(dataDir, "config.json");
  const statePath = process.env.BOT_STATE ?? join(dataDir, "checkpoint.json");

  const config = ConfigStore.load(configPath);
  // The environment pins these for this process only: a deployment can inject a
  // secret without writing it to disk, and the operator's stored setting is
  // never clobbered by the seed values.
  if (process.env.CHAT_URL) config.override("chat.url", process.env.CHAT_URL);
  if (process.env.BOT_TOKEN)
    config.override("chat.token", process.env.BOT_TOKEN);

  const log = (level: LogLevel, message: string): void => {
    if (LEVELS[level] < LEVELS[config.get("log.level")]) return;
    const line = `${new Date().toISOString().slice(11, 19)} ${level.toUpperCase().padEnd(5)} ${message}`;
    if (level === "warn" || level === "error")
      process.stderr.write(`${line}\n`);
    else process.stdout.write(`${line}\n`);
  };

  const store = Store.load(statePath);
  store.taskLimit = config.get("chat.stateLimit");

  // Mutable transport state: a config change rebuilds these in place.
  let online: string[] = [];
  let reconnects = 0;
  let connectedBefore = false;
  let transportChange = Promise.resolve();
  let api = buildApi();
  let model = buildModel();
  let agent: Agent;
  let socket: BotSocket;

  function buildApi(): BotApi {
    return new BotApi({
      baseUrl: config.get("chat.url"),
      token: config.get("chat.token"),
      log: (level, message) => log(level, `${SESSION} ${message}`),
    });
  }

  function buildModel(): ModelClient {
    return new ModelClient({
      endpoint: config.get("model.endpoint"),
      token: config.get("model.token"),
      model: config.get("model.name"),
      timeoutMs: config.get("model.timeoutMs"),
      stream: config.get("model.stream"),
      log,
    });
  }

  function buildAgent(): Agent {
    return new Agent({
      api,
      store,
      config,
      model,
      log,
      presence: () => online,
      socketReady: () => socket?.connected ?? false,
      setActivity: (conversationId, state) =>
        socket?.activity(conversationId, state),
    });
  }

  let transportScheduled = false;
  let transportUrl = config.get("chat.url");
  let transportToken = config.get("chat.token");
  function scheduleTransportChange(): void {
    if (transportScheduled) return;
    transportScheduled = true;
    setImmediate(() => {
      transportChange = transportChange
        .then(async () => {
          // Let the command acknowledgement finish on the existing transport.
          while (agent.busy)
            await new Promise((resolve) => setTimeout(resolve, 10));
          await agent.stop();
          socket.close();
          online = [];
          api = buildApi();
          model = buildModel();
          agent = buildAgent();
          transportUrl = config.get("chat.url");
          transportToken = config.get("chat.token");
          connect();
        })
        .catch((error) =>
          log("error", `transport change failed: ${(error as Error).message}`),
        )
        .finally(() => {
          transportScheduled = false;
        });
    });
  }

  /** Only one socket may own command discovery for an account. */
  function connect(): void {
    const endpoint = api.socket();
    const events = createSocketEvents({
      store,
      log,
      agent: () => agent,
      socket: () => socket,
    });
    socket = new BotSocket({
      ...endpoint,
      supportsActivity: () => api.supportsActivity(),
      log,
      events: {
        ...events,
        status(connected, detail) {
          if (connected) {
            if (connectedBefore) reconnects++;
            connectedBefore = true;
          }
          events.status(connected, detail);
        },
        // The wiring ignores presence; this worker keeps the directory for /presence.
        presence(names: string[]) {
          online = names;
        },
      },
    });
    socket.open();
  }

  agent = buildAgent();
  config.onReload(() => {
    socket.commandsChanged();
    model = buildModel();
    agent.setModel(model);
    store.taskLimit = config.get("chat.stateLimit");
    if (
      config.get("chat.url") !== transportUrl ||
      config.get("chat.token") !== transportToken
    )
      scheduleTransportChange();
  });
  connect();

  // Signals are advisory: catch up after a missed notification or failed fetch.
  setInterval(() => {
    if (stopping) return;
    void agent
      .requestSync()
      .catch((error) =>
        log("debug", `catch-up deferred: ${(error as Error).message}`),
      );
  }, 30_000).unref();

  // --------------------------------------------------------------- lifecycle

  let stopping = false;
  function shutdown(reason: string): void {
    if (stopping) return;
    stopping = true;
    log("info", `${reason} received; saving checkpoint`);
    try {
      store.metrics.reconnects = reconnects;
      store.save();
    } catch (error) {
      log("error", `checkpoint save failed: ${(error as Error).message}`);
    }
    socket?.close();
    process.exitCode = 0;
    setTimeout(() => process.exit(0), 50).unref();
  }
  process.on("SIGINT", () => shutdown("SIGINT"));
  process.on("SIGTERM", () => shutdown("SIGTERM"));
  process.on("SIGHUP", () => {
    try {
      config.reload();
    } catch (error) {
      log("warn", `config reload rejected: ${(error as Error).message}`);
      return;
    }
    log("info", "SIGHUP received; configuration reloaded, resyncing");
    void agent
      .requestSync()
      .catch((error) =>
        log("warn", `sync failed: ${(error as Error).message}`),
      );
  });

  // ------------------------------------------------------------------ banner

  log("info", "CommonRoom simple bot starting");
  log("info", `checkpoint ${statePath}`);
  log("info", `config     ${configPath}`);
  log(
    "info",
    `transport  ${config.get("chat.url")} (token ${config.get("chat.token").length === 0 ? "unset" : "set"})`,
  );
  log(
    "info",
    model.configured
      ? `model      ${model.describe} (stream=${config.get("model.stream")})`
      : "model      not configured; set model.endpoint and model.name before answering prose",
  );
  if (config.get("chat.token").length === 0) {
    log(
      "warn",
      "chat.token is unset; set BOT_TOKEN or use the local config CLI",
    );
  }
  if (config.created)
    log(
      "info",
      `created ${configPath} with defaults; edit it or use the local config CLI`,
    );
  log(
    "debug",
    `effective settings (file ${existsSync(configPath) ? "present" : "absent, using defaults"}): ` +
      Object.entries(config.all())
        .map(
          ([key, value]) =>
            `${key}=${key === "prompt" ? "<configured>" : redact(key as ConfigKey, value)}`,
        )
        .join(" "),
  );

  // A periodic, non-fatal progress line keeps a long-running worker observable.
  setInterval(() => {
    const metrics = store.metrics;
    log(
      "info",
      `status syncs=${metrics.syncs} frames=${metrics.frames} sent=${metrics.sent} replays=${metrics.replayed} ` +
        `revisions=${metrics.revisions} edits=${metrics.edits} retractions=${metrics.retractions} ` +
        `conversations=${agent.directory?.conversations.length ?? 0} reconnects=${reconnects}`,
    );
  }, 300_000).unref();

  // Run one HTTP sync immediately: this surfaces credential and URL problems
  // even when the socket cannot connect.
  void (async () => {
    try {
      const snapshot = await api.me();
      await agent.handleSnapshot(snapshot);
      log(
        "info",
        `authenticated as ${snapshot.account.username} (${snapshot.account.id})`,
      );
      if (snapshot.conversations.length === 0) {
        log(
          "warn",
          "no readable conversations: add the bot to a room (/add <bot> <room>) or create a direct conversation " +
            "(`/dm <bot>`); an empty checkpoint means nothing is visible to this credential",
        );
      } else {
        log(
          "info",
          `${snapshot.conversations.length} conversation(s): ` +
            snapshot.conversations
              .map((conversation) => conversation.view)
              .join(", "),
        );
      }
    } catch (error) {
      if (error instanceof BotApiError && error.code === "unauthorized") {
        log(
          "error",
          `credential rejected (${error.status}); check chat.token and credential expiry. ` +
            "The socket will keep retrying; the worker recovers automatically once a valid " +
            "`chat.token` arrives through a configuration change.",
        );
      } else {
        log("warn", `initial sync failed: ${(error as Error).message}`);
      }
    }
  })();
}

main();
