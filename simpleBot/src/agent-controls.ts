import { isConfigKey, quotedValue, formatConfig } from "./config.ts";
import type { AgentState } from "./agent-state.ts";
import type { BotCommand, Conversation, Message } from "./wire.ts";
import type { TaskRecord } from "./store.ts";
const PUBLIC_COMMANDS: BotCommand[] = [
  {
    name: "/metrics",
    usage: "/metrics",
    description:
      "Reply latency, model usage and cache metrics for this conversation",
  },
  { name: "/help", usage: "/help", description: "How to chat with this bot" },
  {
    name: "/ping",
    usage: "/ping",
    description: "Check that the worker is available",
  },
];
const WORKER_COMMANDS: BotCommand[] = [
  {
    name: "/reload",
    usage: "/reload",
    description: "Reload validated settings from the worker configuration file",
    operator_only: true,
  },
  {
    name: "/admin",
    usage: "/admin",
    description: "List this bot's owner/admin controls",
    operator_only: true,
  },
  {
    name: "/whoami",
    usage: "/whoami",
    description: "Show this bot's identity and owner",
    operator_only: true,
  },
  {
    name: "/owner",
    usage: "/owner",
    description: "Show who owns this bot",
    operator_only: true,
  },
  {
    name: "/status",
    usage: "/status",
    description: "Inspect this worker's status",
    operator_only: true,
  },
  {
    name: "/features",
    usage: "/features",
    description: "Inspect this worker's implementation features",
    operator_only: true,
  },
  {
    name: "/model",
    usage: "/model",
    description: "Inspect model configuration",
    operator_only: true,
  },
  {
    name: "/config",
    usage: '/config get [key] | set key "value"',
    description: "Read or change this bot's configuration",
    operator_only: true,
    subcommands: [
      {
        name: "get",
        usage: "/config get [key]",
        description: "Read configuration with secrets redacted",
      },
      {
        name: "set",
        usage: '/config set key "value"',
        description: "Change a model or worker setting",
      },
    ],
  },
];

export class Controls {
  private readonly host: AgentState;
  constructor(host: AgentState) {
    this.host = host;
  }
  recognizes(conversation: Conversation, message: Message): boolean {
    const account = this.host.snapshot?.account;
    if (
      !account ||
      !(
        message.mentions.includes(account.username) ||
        conversation.view.startsWith("@direct:")
      )
    )
      return false;
    const body = message.text.startsWith(`@${account.username}`)
      ? message.text.slice(account.username.length + 1).trim()
      : message.text.trim();
    return [...PUBLIC_COMMANDS, ...WORKER_COMMANDS].some(
      (command) => command.name === body.split(/\s/u)[0]?.toLowerCase(),
    );
  }
  async commands(conversationId: string): Promise<BotCommand[]> {
    const credential = this.host.snapshot?.credential;
    if (!this.host.snapshot?.conversations.some((c) => c.id === conversationId))
      return [];
    const privileged =
      credential?.operations === null ||
      credential?.operations.includes("config");
    return [...PUBLIC_COMMANDS, ...(privileged ? WORKER_COMMANDS : [])];
  }

  /** Read-only diagnostics for the worker's owner/admin chat commands. */
  operatorValues(): Record<string, string> {
    const conversation = this.host.snapshot?.conversations[0];
    const config = this.host.options.config;
    return {
      "worker.help":
        "Admin worker diagnostics: use @BOT /status, /features or /model. Chat requests use the bot account's server commands and grants.",
      "worker.status": conversation
        ? this.status(conversation)
        : "No readable conversation is available.",
      "worker.features": conversation
        ? this.features(conversation)
        : "No readable conversation is available.",
      "worker.model": `Model: ${this.host.options.model.describe}\nStreaming: ${config.get("model.stream")}\nOutput token limit: ${config.get("model.maxTokens")}\nReasoning effort: ${config.get("model.reasoningEffort") || "provider default"}\nTimeout: ${config.get("model.timeoutMs")} ms`,
    };
  }
  private features(conversation: Conversation): string {
    const { store, config } = this.host.options;
    const credential = this.host.snapshot?.credential;
    const operations =
      credential?.operations?.join(", ") ?? "all (account-wide credential)";
    return [
      "**Live in this worker**",
      "- Keyed idempotent sends with a durable outbox and stable per-output slots",
      "- Multi-part output: text over 4,000 characters is split and numbered",
      "- Replies (`reply_to`) when the account holds `/reply`, plus reactions",
      "- Revision reconciliation: edits and retractions trigger a pinned rescan",
      "- Transient `working`/`idle` activity signals",
      "- Event-driven one-call ticks in rooms and DMs, with model-chosen continuation",
      "- Concurrent conversations and durable background jobs; fixed 3-second room batches",
      "- Persistent KV memory and a shared read/write data/workspace filesystem",
      "- Network-enabled JS/TS execution in restricted, bounded Deno subprocesses",
      "- Owner/admin configuration through @Bot /config, with redacted values",
      "- Presence directory (account-wide credentials only)",
      `- Operations on this credential: ${operations}`,
      "",
      "**Around it**",
      "- `POST /credentials` issues scoped, expiring worker tokens (operator action)",
      "- `POST /conversations` creates rooms or direct pairs when granted",
      "- Read acknowledgement through an explicitly enabled model tool",
      "- History cursor modes: after, before, around, start/end, revision-pinned",
      "",
      `Conversation \`${conversation.view}\`: revision ${conversation.content_revision}, ` +
        `unread.through ${conversation.unread?.through ?? "unknown"}, ` +
        `autonomous=${config.get("autonomous")}, respondToAllDms=${config.get("chat.respondToAllDms")}.`,
      `This session: ${store.metrics.syncs} syncs, ${store.metrics.edits} edits, ${store.metrics.retractions} retractions, ${store.metrics.sent} messages sent (${store.metrics.replayed} replays).`,
    ].join("\n");
  }

  private status(conversation: Conversation): string {
    const { store, config } = this.host.options;
    const credential = this.host.snapshot?.credential;
    const pending = store
      .tasks()
      .filter(
        (task) => task.status === "queued" || task.status === "running",
      ).length;
    const conversations = this.host.snapshot?.conversations ?? [];
    const cursor = store.cursor(conversation.id) ?? 0;
    const session = store.state.sessions[conversation.id];
    const uptime = Math.round((Date.now() - this.host.startedAt) / 1000);
    return [
      `Account: ${this.host.snapshot?.account.username} (${this.host.snapshot?.account.id})`,
      `Credential: ${credential?.id ?? "unknown"}${credential?.expires ? ` expires ${new Date(credential.expires * 1000).toISOString()}` : " (no expiry)"}`,
      `Conversation scopes: ${credential?.conversations === null || credential?.conversations === undefined ? "account-wide" : `${credential.conversations.length}`}`,
      `Operations: ${credential?.operations?.join(", ") ?? "account-wide"}`,
      `Directory: ${conversations.length} readable conversation(s)`,
      `This conversation: ${conversation.view} revision ${conversation.content_revision}, cursor ${cursor}, retained from ${conversation.unread?.oldest ?? "?"}`,
      ...(session
        ? [
            `Tick: ${session.tick}; phase ${session.phase}; next ${session.next.kind}; episode calls ${session.calls}/${config.get("agent.maxTicks")}; tokens ${session.tokens ?? 0}/${config.get("agent.maxEpisodeTokens")}; jobs ${Object.keys(session.jobs).length}`,
            ...(session.error ? [`Last tick error: ${session.error}`] : []),
          ]
        : []),
      `Shell access: ${this.host.shell.enabled ? "enabled (--with-shell)" : "disabled"} (startup-only capability)`,
      `Pending tasks: ${pending} · socket ${this.host.options.socketReady() ? "connected" : "reconnecting"}`,
      `Config: model ${config.get("model.name")}, stream=${config.get("model.stream")}, autonomous=${config.get("autonomous")}, respondToAllDms=${config.get("chat.respondToAllDms")}, acknowledgeReads=${config.get("chat.acknowledgeReads")}`,
      `Uptime: ${uptime}s · syncs ${store.metrics.syncs} · edits ${store.metrics.edits} · retractions ${store.metrics.retractions} · sent ${store.metrics.sent} · replays ${store.metrics.replayed}`,
    ].join("\n");
  }

  async handle(
    conversation: Conversation,
    message: Message,
    task: TaskRecord,
  ): Promise<string | undefined> {
    const prefix = `@${this.host.snapshot?.account.username ?? "bot"}`;
    const body =
      message.text.startsWith(prefix) &&
      /^\s/u.test(message.text.slice(prefix.length))
        ? message.text.slice(prefix.length).trim()
        : message.text.trim();
    const name = (body.split(/\s/u)[0] ?? "").toLowerCase();
    if (PUBLIC_COMMANDS.some((command) => command.name === name)) {
      const response =
        name === "/ping"
          ? "Pong."
          : name === "/metrics"
            ? this.host.metrics(conversation.id).report({
                compactTokens: this.host.options.config.get(
                  "model.compactTokens",
                ),
                contextTokens: this.host.options.config.get(
                  "model.contextTokens",
                ),
                batchWindowMs:
                  this.host.options.config.get("chat.batchWindowMs"),
              })
            : "Mention me or send a direct message to ask a question. Reply to an older message to include its context; I can fetch more history when needed. Public commands: /help, /ping, /metrics.";
      return response;
    }
    if (WORKER_COMMANDS.some((command) => command.name === name)) {
      task.privateCommand = true;
      await this.host.options.store.save();
      let response: string;
      try {
        const authorization = await this.host.options.api.messageContext(
          conversation.id,
          message.id,
        );
        if (
          authorization.bot.id !== this.host.snapshot?.account.id ||
          authorization.author.id !== message.author_id
        )
          throw new Error(
            "Source identity did not match the authenticated message.",
          );
        if (
          !authorization.author.groups.includes("su") &&
          (authorization.bot.groups.includes("su") ||
            (!authorization.author.groups.includes("admin") &&
              authorization.author.id !== authorization.bot.owner_id))
        )
          response =
            "Only this bot's owner or a server administrator can use its private commands.";
        else if (name === "/whoami")
          response = `I am ${this.host.snapshot?.account.username} (${this.host.snapshot?.account.id}). Owner: ${authorization.bot.owner ?? "server administrators"}.`;
        else if (name === "/owner")
          response = `Owner: ${authorization.bot.owner ?? "server administrators"}.`;
        else if (name === "/admin")
          response = WORKER_COMMANDS.map(
            (command) =>
              `- \`@${this.host.snapshot?.account.username} ${command.usage}\` — ${command.description}`,
          ).join("\n");
        else if (name === "/config") {
          const argument = body.slice(name.length).trim();
          const update = /^set\s+(\S+)(?:\s+([\s\S]*))?$/u.exec(argument);
          if (update) {
            const key = update[1];
            if (!key || !isConfigKey(key))
              throw new Error(`Unknown configuration key ${key}`);
            response = formatConfig(
              this.host.options.config.apply(key, quotedValue(update[2] ?? "")),
            );
          } else if (/^get(?:\s|$)/u.test(argument))
            response = formatConfig(
              this.host.options.config.fetch(
                argument.slice(3).trim() === "."
                  ? null
                  : argument.slice(3).trim() || null,
              ),
            );
          else response = 'Usage: /config get [key] | /config set key "value"';
        } else if (name === "/reload") {
          this.host.options.config.reload();
          response =
            "Configuration reloaded.\n" +
            formatConfig(this.host.options.config.fetch(null));
        } else
          response =
            this.operatorValues()[`worker.${name.slice(1)}`] ??
            "No report is available.";
      } catch (error) {
        response =
          error instanceof Error ? error.message : "Private command failed";
      }
      return response;
    }
    return undefined;
  }
}
