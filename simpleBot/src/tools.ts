import { READ_JOB_TOOL } from "./job-results.ts";
import type { FunctionTool } from "./model.ts";
import { FINISH_TURN_TOOL } from "./group.ts";
import { EXECUTION_TOOL } from "./execution.ts";
import { MEMORY_TOOL } from "./memory.ts";
import type { ToolAccess } from "./tool-locks.ts";

/** All classifications are host-owned, including unknown/malformed calls. */
export function toolAccess(
  name: string,
  args: string,
  conversation: string,
  workspace: string,
): ToolAccess {
  if (name === "read_job")
    return { resource: `jobs:${conversation}`, mode: "read" };
  if (name === "memory") {
    let read = false;
    try {
      const input: unknown = JSON.parse(args);
      read =
        typeof input === "object" &&
        input !== null &&
        "action" in input &&
        (input.action === "get" || input.action === "list");
    } catch {
      // Malformed requests take the exclusive path before tool validation.
    }
    return {
      resource: `memory:${conversation}`,
      mode: read ? "read" : "write",
    };
  }
  if (name === "execute_code")
    return { resource: `workspace:${workspace}`, mode: "write" };
  // History reads also mutate the turn's context budget and deduplication cache.
  if (name === "read_history")
    return { resource: `history:${conversation}`, mode: "write" };
  // Commands may affect other rooms or account-wide state; keep one writer.
  return { resource: "server-actions", mode: "write" };
}

export function modelTools(options: {
  group: boolean;
  commands: boolean;
  history: boolean;
  reads: boolean;
  ticks?: boolean;
}): FunctionTool[] {
  const {
    group: batch,
    commands,
    history: historyEnabled,
    reads: readsEnabled,
  } = options;
  return [
    MEMORY_TOOL,
    ...(options.ticks ? [READ_JOB_TOOL] : []),
    EXECUTION_TOOL,
    ...(batch ? [FINISH_TURN_TOOL] : []),
    ...(commands
      ? [
          {
            type: "function" as const,
            function: {
              name: "run_server_command",
              description:
                "Run an allowed CommonRoom server command as the bot account in the current conversation. The server checks all grants.",
              parameters: {
                type: "object",
                properties: {
                  command: {
                    type: "string",
                    description:
                      "Complete slash command using the server catalogue syntax",
                  },
                },
                required: ["command"],
                additionalProperties: false,
              },
            },
          },
        ]
      : []),
    ...(historyEnabled
      ? [
          {
            type: "function" as const,
            function: {
              name: "read_history",
              description:
                "Read recent messages in the current conversation, excluding worker controls. Use before or after sequence cursors to page.",
              parameters: {
                type: "object",
                properties: {
                  message_id: {
                    type: "string",
                    description: "Load the page around a retained message ID",
                  },
                  around: { type: "integer", minimum: 1 },
                  before: { type: "integer", minimum: 1 },
                  after: { type: "integer", minimum: 0 },
                  limit: {
                    type: "integer",
                    minimum: 1,
                    maximum: 20,
                  },
                },
                additionalProperties: false,
              },
            },
          },
        ]
      : []),
    ...(readsEnabled
      ? [
          {
            type: "function" as const,
            function: {
              name: "acknowledge_read",
              description:
                "Acknowledge this conversation through the current input message, clearing the bot account's shared unread cursor. Enabled explicitly by its operator.",
              parameters: {
                type: "object",
                properties: {},
                additionalProperties: false,
              },
            },
          },
        ]
      : []),
  ];
}
