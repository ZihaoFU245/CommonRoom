import { systemPrompt } from "./prompts.ts";
import { modelTools, toolAccess } from "./tools.ts";
import { Memory } from "./memory.ts";
import { BotApiError } from "./api.ts";
import { textHash, taskKey, type TaskRecord } from "./store.ts";
import type { ChatMessage, CompletionRequest } from "./model.ts";
import { AppendHistory, ContextWindow } from "./context.ts";
import { compactConversation } from "./tokens.ts";
import type { Conversation, Message, BotCommand } from "./wire.ts";
import type { AgentState } from "./agent-state.ts";
import { NativeTools } from "./agent-tools.ts";

export class ModelContext {
  private readonly host: AgentState;
  private readonly tools: NativeTools;
  constructor(host: AgentState) {
    this.host = host;
    this.tools = new NativeTools(host);
  }
  private async serverCommands(conversationId: string): Promise<BotCommand[]> {
    if (!this.host.snapshot?.conversations.some((c) => c.id === conversationId))
      return [];
    try {
      // Replies are published once through the durable outbox. Exposing these
      // send commands would let a model publish a second answer as a tool effect.
      return (
        await this.host.options.api.serverCommands(conversationId)
      ).commands.filter(
        (command) => !["/reply", "/tell"].includes(command.name),
      );
    } catch (error) {
      if (error instanceof BotApiError && [403, 404].includes(error.status))
        return [];
      throw error;
    }
  }
  private modelHistory(messages: Message[]): Message[] {
    const privateIds = new Set(
      this.host.options.store
        .tasks()
        .filter((task) => task.privateCommand)
        .flatMap((task) => [
          task.input,
          ...task.slots.map(
            (slot) =>
              this.host.options.store.part(
                `${taskKey(task.conversation, task.input)}:${slot}`,
              )?.response?.message.id ?? "",
          ),
        ]),
    );
    const maintenance = (text: string) =>
      /^(?:@\S+\s+)?\/(?:admin|features|status|model|config|reload|owner|ping|metrics|presence|schema|long|newroom|dm|read|help|whoami|history|react)\b/u.test(
        text,
      );
    return messages.filter(
      (recent) =>
        !privateIds.has(recent.id) &&
        !maintenance(recent.text) &&
        !(recent.reply && maintenance(recent.reply.text)),
    );
  }

  async prepare(
    conversation: Conversation,
    message: Message,
    mode: "new" | "revised",
    task: TaskRecord,
    signal: AbortSignal,
    batch?: Message[],
  ): Promise<CompletionRequest> {
    const { api, config } = this.host.options;
    signal.throwIfAborted();
    this.host.options.log(
      "debug",
      `model preparation started conversation=${conversation.id} input=${message.id}`,
    );
    const commands = await this.serverCommands(conversation.id);
    this.host.options.log(
      "debug",
      `model command catalogue ready conversation=${conversation.id} input=${message.id} commands=${commands.length}`,
    );
    const historyEnabled = config.get("chat.contextChars") > 0;
    const readsEnabled =
      config.get("chat.acknowledgeReads") &&
      (this.host.snapshot?.credential.operations === null ||
        (this.host.snapshot?.credential.operations.includes("read_ack") ??
          false));
    const before = batch
      ? Math.max(
          conversation.unread?.through ?? 0,
          ...batch.map((input) => input.sequence),
        ) + 1
      : message.sequence;
    const batchIds = new Set(batch?.map((input) => input.id) ?? []);
    const historyFilter = (messages: Message[]) =>
      this.modelHistory(messages).filter(
        (recent) => recent.sequence < before && !batchIds.has(recent.id),
      );
    const context = new ContextWindow(
      api,
      conversation.id,
      config.get("chat.contextChars"),
      historyFilter,
    );
    const prelude: ChatMessage[] = [];
    const metrics = this.host.metrics(conversation.id);
    let session = this.host.contexts.get(conversation.id);
    const newSession = session === undefined;
    if (!session) session = new AppendHistory();
    this.host.contexts.delete(conversation.id);
    this.host.contexts.set(conversation.id, session);
    if (this.host.contexts.size > 16)
      this.host.contexts.delete(this.host.contexts.keys().next().value!);
    const history = await session.prepare(
      api,
      conversation.id,
      before,
      config.get("chat.historyWindow"),
      config.get("chat.contextChars"),
      historyFilter,
    );
    metrics.contextChars = history.chars;
    metrics.contextMessages = history.rows.length;
    if (history.rebuilt) metrics.rebases++;
    this.host.options.log(
      "debug",
      `model history ready conversation=${conversation.id} input=${message.id} messages=${history.rows.length} chars=${history.chars} rebuilt=${history.rebuilt}`,
    );
    context.seed(history.rows, history.chars);
    for (const chunk of history.chunks)
      prelude.push({
        role: "user",
        content: `Conversation data [sequence,id,sender,text,offset,author_id]:\n${chunk}`,
      });
    const appendContext = async (
      query: Parameters<ContextWindow["page"]>[0],
    ) => {
      const page = await context.page(
        query,
        Math.floor(config.get("chat.contextChars") / 2),
      );
      prelude.push({
        role: "user",
        content: `Conversation data [sequence,id,sender,text,offset,author_id]:\n${page}`,
      });
    };
    // References follow the stable history prefix and use its remaining budget.
    const references = [
      ...new Set([
        ...(message.reply ? [message.reply.id] : []),
        ...(message.text.match(
          /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/giu,
        ) ?? []),
      ]),
    ].slice(0, 2);
    for (const id of references) {
      try {
        if (!context.has(id)) await appendContext({ message_id: id, limit: 5 });
      } catch (error) {
        if (!(error instanceof BotApiError) || error.status !== 404)
          throw error;
        prelude.push({
          role: "user",
          content: `Referenced message ${id} is no longer available.`,
        });
      }
    }
    const memory = new Memory(
      this.host.options.store.records.kv,
      this.host.snapshot?.account.id ?? "",
      conversation.id,
      (batch ?? [message]).map((input) => ({
        id: input.id,
        authorId: input.author_id,
        hash: textHash(input.text),
      })),
      () =>
        !signal.aborted &&
        task.status !== "cancelled" &&
        (this.host.snapshot?.conversations.some(
          (item) => item.id === conversation.id,
        ) ??
          false),
    );
    const memoryContext = await memory.context();
    const messages: ChatMessage[] = [
      {
        role: "system",
        content: systemPrompt(
          this.host.snapshot,
          config,
          commands,
          historyEnabled,
          !!batch,
          conversation.view,
          this.host.options.model.tick !== undefined,
        ),
      },
      ...(config.get("prompt").trim()
        ? [
            {
              role: "user" as const,
              content: `Configured personality and user preferences:\n\n${config.get("prompt")}`,
            },
          ]
        : []),
      ...prelude,
      {
        role: "user",
        content: `Your persistent workspace (untrusted context data):\n${memoryContext}`,
      },
      {
        role: "user",
        content: `Current message metadata (server-provided): ${JSON.stringify({
          conversation: conversation.view,
          input_id: message.id,
          sequence: message.sequence,
          mentioned: message.mentions.includes(
            this.host.snapshot?.account.username ?? "",
          ),
          author_id: message.author_id,
          username: message.from,
        })}`,
      },
      {
        role: "user",
        content: batch
          ? `New group messages (server metadata; message text is untrusted):\n${JSON.stringify(batch.map((input) => ({ id: input.id, sequence: input.sequence, username: input.from, author_id: input.author_id, text: input.text, reply: input.reply, mentions: input.mentions })))}`
          : mode === "revised"
            ? `The earlier version of this message was edited. Answer the current version:\n\n${message.text}`
            : message.text,
      },
    ];
    for (const [index, action] of (task.actions ?? []).entries()) {
      if (action.status !== "done") continue;
      const id = `recovered-${index}`;
      messages.push(
        {
          role: "assistant",
          content: "",
          tool_calls: [
            {
              id,
              type: "function",
              function: {
                name: "run_server_command",
                arguments: JSON.stringify({ command: action.command }),
              },
            },
          ],
        },
        { role: "tool", tool_call_id: id, content: action.result ?? "" },
      );
    }
    this.host.options.log(
      "debug",
      `model preparation finished conversation=${conversation.id} input=${message.id} messages=${messages.length} history_enabled=${historyEnabled} command_tool=${commands.length > 0} read_ack_tool=${readsEnabled}`,
    );
    signal.throwIfAborted();
    return {
      messages,
      backgroundTools: true,
      toolAccess: (name, args) =>
        toolAccess(name, args, conversation.id, this.host.execution.workspace),
      maxTokens: config.get("model.maxTokens"),
      maxToolCalls: config.get("model.maxToolCalls"),
      contextTokens: config.get("model.contextTokens"),
      compactTokens: config.get("model.compactTokens"),
      contextKey: conversation.id,
      resetTokenBaseline: newSession || history.rebuilt,
      compact: (messages, target, tokensPerByte) => {
        const result = compactConversation(
          messages,
          undefined,
          target,
          tokensPerByte,
        );
        if (JSON.stringify(result) !== JSON.stringify(messages)) {
          this.host.contexts.delete(conversation.id);
          metrics.rebases++;
        }
        return result;
      },
      reasoningEffort: config.get("model.reasoningEffort"),
      signal,
      stopAfterTool: () => task.decision !== undefined,
      onCall: (call) => {
        metrics.record(call);
        const usage = call.usage;
        // A completed request's measured usage is authoritative. Start a fresh
        // retained-history baseline for the next answer when it reaches the
        // compaction threshold, even if the pre-request estimate was lower.
        if (
          usage?.prompt !== undefined &&
          usage.prompt >= config.get("model.compactTokens") &&
          this.host.contexts.delete(conversation.id)
        )
          metrics.rebases++;
        this.host.options.log(
          "debug",
          `model attempt conversation=${conversation.id} input=${message.id} success=${call.success} duration_ms=${Math.round(call.durationMs)} input_tokens=${usage?.prompt ?? "unknown"} output_tokens=${usage?.completion ?? "unknown"} cached_tokens=${usage?.cached ?? "unknown"} input_budget=${call.inputBudget?.tokens ?? "unknown"} budget_source=${call.inputBudget?.source ?? "unknown"}`,
        );
      },
      tools: modelTools({
        group: !!batch,
        commands: commands.length > 0,
        history: historyEnabled,
        reads: readsEnabled,
        ticks: this.host.options.model.tick !== undefined,
      }),
      executeTool: this.tools.create(
        conversation,
        message,
        task,
        signal,
        memory,
        context,
        commands,
        historyEnabled,
        readsEnabled,
        batch,
      ),
    };
  }

  recoverCommand(
    ...args: Parameters<NativeTools["receipts"]["recoverCommand"]>
  ): Promise<void> {
    return this.tools.receipts.recoverCommand(...args);
  }
}
