import { readJob } from "./job-results.ts";
import type { AgentState } from "./agent-state.ts";
import type { CompletionRequest } from "./model.ts";
import type { Conversation, Message, BotCommand } from "./wire.ts";
import type { TaskRecord, CommandAction } from "./store.ts";
import type { ContextWindow } from "./context.ts";
import type { Memory } from "./memory.ts";
import { BotApiError, idempotencyKey } from "./api.ts";
import { groupResponses } from "./group.ts";
import { preview } from "./text.ts";

export class NativeTools {
  private readonly host: AgentState;
  readonly receipts: CommandReceipts;
  constructor(host: AgentState) {
    this.host = host;
    this.receipts = new CommandReceipts(host);
  }
  create(
    conversation: Conversation,
    message: Message,
    task: TaskRecord,
    signal: AbortSignal,
    memory: Memory,
    context: ContextWindow,
    commands: BotCommand[],
    historyEnabled: boolean,
    readsEnabled: boolean,
    batch?: Message[],
  ): NonNullable<CompletionRequest["executeTool"]> {
    const api = this.host.options.api;
    return async (
      name: string,
      argumentsText: string,
      toolSignal?: AbortSignal,
      effectId?: string,
    ): Promise<string> => {
      const activeSignal = toolSignal
        ? AbortSignal.any([signal, toolSignal])
        : signal;
      activeSignal.throwIfAborted();
      if (name === "read_job")
        return readJob(
          this.host.options.store.state.sessions[conversation.id],
          argumentsText,
        );
      if (name === "memory") return memory.execute(argumentsText);
      if (name === "execute_code") {
        if (
          this.host.stopped ||
          !this.host.snapshot?.conversations.some(
            (item) => item.id === conversation.id,
          )
        )
          return JSON.stringify({
            error: "Execution turn is no longer active",
          });
        return this.host.execution.execute(argumentsText, activeSignal);
      }
      if (name === "finish_turn" && batch) {
        try {
          task.decision = groupResponses(
            argumentsText,
            batch,
            conversation.permissions.reply,
          );
        } catch (error) {
          return JSON.stringify({
            error:
              error instanceof Error ? error.message : "Invalid response plan",
          });
        }
        await this.host.options.store.save();
        return JSON.stringify({
          finished: true,
          messages: task.decision.length,
        });
      }
      if (name === "read_history" && historyEnabled) {
        let input: unknown;
        try {
          input = JSON.parse(argumentsText);
        } catch {
          return JSON.stringify({ error: "Invalid arguments" });
        }
        if (
          typeof input !== "object" ||
          input === null ||
          Array.isArray(input) ||
          Object.entries(input).some(([key, value]) =>
            key === "message_id"
              ? typeof value !== "string" ||
                !/^[A-Za-z0-9._:-]{1,128}$/.test(value)
              : !["before", "after", "around", "limit"].includes(key) ||
                typeof value !== "number" ||
                !Number.isSafeInteger(value) ||
                value < (key === "after" ? 0 : 1) ||
                (key === "limit" && value > 20),
          )
        )
          return JSON.stringify({
            error: "Expected bounded history cursor arguments",
          });
        const query = input as {
          before?: number;
          after?: number;
          around?: number;
          message_id?: string;
          limit?: number;
        };
        if (
          (query.message_id &&
            Object.keys(query).some(
              (key) => !["message_id", "limit"].includes(key),
            )) ||
          [query.before, query.after, query.around].filter(
            (value) => value !== undefined,
          ).length > 1
        )
          return JSON.stringify({
            error: "Choose one cursor or message_id",
          });
        try {
          return await context.page(
            {
              ...query,
              limit: query.limit ?? 6,
            },
            undefined,
            activeSignal,
          );
        } catch (error) {
          if (error instanceof BotApiError && [400, 404].includes(error.status))
            return JSON.stringify({ error: error.message });
          throw error;
        }
      }
      if (name === "acknowledge_read" && readsEnabled) {
        let input: unknown;
        try {
          input = JSON.parse(argumentsText);
        } catch {
          return JSON.stringify({ error: "Invalid arguments" });
        }
        if (
          typeof input !== "object" ||
          input === null ||
          Array.isArray(input) ||
          Object.keys(input).length
        )
          return JSON.stringify({
            error: "Expected an empty argument object",
          });
        if (task.status === "cancelled" || this.host.abort.signal.aborted)
          throw new Error("Task cancelled before read acknowledgement");
        return JSON.stringify(
          await api.acknowledgeRead(
            conversation.id,
            message.sequence,
            activeSignal,
          ),
        );
      }
      if (name !== "run_server_command" || argumentsText.length > 8192)
        return JSON.stringify({
          error: "Unknown tool or oversized arguments",
        });
      let input: unknown;
      try {
        input = JSON.parse(argumentsText);
      } catch {
        return JSON.stringify({ error: "Invalid JSON arguments" });
      }
      if (
        typeof input !== "object" ||
        input === null ||
        Array.isArray(input) ||
        Object.keys(input).length !== 1 ||
        !("command" in input) ||
        typeof input.command !== "string"
      )
        return JSON.stringify({
          error: "Expected only a command string",
        });
      const command = input.command.trim();
      if (
        command.length > 4100 ||
        !commands.some((c) => c.name === command.split(/\s/u)[0])
      )
        return JSON.stringify({
          error: "Command is not in the available server catalogue",
        });
      if (/^(?:\/sudo\s+)?\/(?:reply|tell)(?:\s|$)/u.test(command))
        return JSON.stringify({
          error:
            "Return your answer as the final response; replies are sent once through the worker outbox",
        });
      if (/^(?:\/sudo\s+)?\/history(?:\s|$)/u.test(command))
        return JSON.stringify({
          error: "Use read_history for conversation history",
        });
      if (task.status === "cancelled" || this.host.abort.signal.aborted)
        throw new Error("Task was cancelled before command execution");
      let action = task.actions?.find((action) =>
        effectId ? action.effectId === effectId : action.command === command,
      );
      if (!action) {
        if ((task.actions?.length ?? 0) >= 16)
          return JSON.stringify({
            error: "Task action limit reached",
          });
        action = {
          command,
          ...(effectId ? { effectId } : {}),
          status: "pending",
          key: idempotencyKey(
            conversation.id,
            message.id,
            effectId
              ? `job:${effectId}`
              : `command:${task.actions?.length ?? 0}`,
          ),
          created: Date.now(),
        };
        (task.actions ??= []).push(action);
        action = task.actions.at(-1)!;
        await this.host.options.store.save();
        activeSignal.throwIfAborted();
        return this.receipts.executeRecordedCommand(
          conversation.id,
          action,
          activeSignal,
        );
      }
      if (action.status === "pending")
        await this.receipts.recoverCommand(
          conversation.id,
          action,
          true,
          activeSignal,
        );
      return action.result ?? "";
    };
  }
}

export class CommandReceipts {
  private readonly host: AgentState;
  constructor(host: AgentState) {
    this.host = host;
  }
  async executeRecordedCommand(
    conversation: string,
    action: CommandAction,
    signal?: AbortSignal,
  ): Promise<string> {
    if (
      !action.key ||
      action.created === undefined ||
      Date.now() - action.created >= 23 * 60 * 60 * 1000
    )
      throw new Error(
        "Command receipt retry window expired or legacy command is unkeyed; inspect before retrying",
      );
    try {
      const value = await this.host.options.api.executeCommand(
        conversation,
        action.command,
        action.key,
        signal,
      );
      action.result = JSON.stringify({ result: preview(value.result, 3000) });
    } catch (error) {
      if (
        !(error instanceof BotApiError) ||
        error.retryable ||
        error.code === "network"
      )
        throw error;
      action.result = JSON.stringify({ error: preview(error.message, 500) });
    }
    action.status = "done";
    await this.host.options.store.save();
    return action.result;
  }
  async recoverCommand(
    conversation: string,
    action: CommandAction,
    submitMissing = true,
    signal?: AbortSignal,
  ): Promise<void> {
    if (!action.key)
      throw new Error("Legacy command is unkeyed; inspect before retrying");
    const receipt = await this.host.options.api.commandReceipt(
      action.key,
      signal,
    );
    if (receipt.state === "not_found") {
      if (submitMissing)
        await this.executeRecordedCommand(conversation, action, signal);
      return;
    }
    action.result = JSON.stringify(
      receipt.state === "succeeded"
        ? { result: preview(receipt.result ?? "", 3000) }
        : { error: receipt.error?.message ?? "Command failed" },
    );
    action.status = "done";
    await this.host.options.store.save();
  }
}
