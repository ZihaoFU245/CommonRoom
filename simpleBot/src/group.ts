// Pure batching bounds and the autonomous turn's terminal response tool.
import type { FunctionTool } from "./model.ts";
import type { Message } from "./wire.ts";
import type { TaskRecord } from "./store.ts";

/** Old v1 group checkpoints retain their original three-second deadline. */
export const LEGACY_GROUP_INTERVAL_MS = 3000;
export const DEFAULT_BATCH_WINDOW_MS = 1000;
export const MAX_BATCH_WINDOW_MS = 10000;
export const MAX_GROUP_INPUTS = 100;
export const MAX_GROUP_CHARS = 16000;
export const MAX_GROUP_RESPONSE_CHARS = 262144;

export const FINISH_TURN_TOOL: FunctionTool = {
  type: "function",
  function: {
    name: "finish_turn",
    description:
      "Finish this group turn. Send one plain message for related questions, separate targeted replies for independent questions, or no messages to stay silent. Never repeat responses in final text.",
    parameters: {
      type: "object",
      properties: {
        messages: {
          type: "array",
          maxItems: 16,
          items: {
            type: "object",
            properties: {
              text: { type: "string", minLength: 1, maxLength: 4000 },
              reply_to: {
                type: "string",
                description:
                  "Optional ID from the current batch; omit for a plain group message",
              },
            },
            required: ["text"],
            additionalProperties: false,
          },
        },
      },
      required: ["messages"],
      additionalProperties: false,
    },
  },
};

export function groupInputs(
  members: TaskRecord[],
  leader: TaskRecord,
): NonNullable<TaskRecord["batch"]> {
  if (!members.includes(leader)) members.unshift(leader);
  const result: NonNullable<TaskRecord["batch"]> = [];
  let size = 0;
  for (const member of members) {
    if (
      result.length >= MAX_GROUP_INPUTS ||
      (result.length > 0 &&
        size + (member.inputChars ?? 4000) > MAX_GROUP_CHARS)
    )
      break;
    result.push({
      input: member.input,
      sequence: member.sequence,
      hash: member.promptHash,
    });
    size += member.inputChars ?? 4000;
  }
  return result;
}

export function groupResponses(
  raw: string,
  inputs: Message[],
  canReply: boolean,
): NonNullable<TaskRecord["decision"]> {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    throw new Error("Invalid JSON");
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    Object.keys(value).length !== 1 ||
    !("messages" in value) ||
    !Array.isArray(value.messages) ||
    value.messages.length > 16
  )
    throw new Error("Expected up to 16 messages");
  const responses: NonNullable<TaskRecord["decision"]> = [];
  for (const item of value.messages as unknown[]) {
    if (
      typeof item !== "object" ||
      item === null ||
      Array.isArray(item) ||
      !("text" in item) ||
      typeof item.text !== "string" ||
      !item.text.trim() ||
      Array.from(item.text).length > 4000 ||
      Object.keys(item).some((key) => !["text", "reply_to"].includes(key))
    )
      throw new Error("Each message needs 1-4000 characters");
    if (
      "reply_to" in item &&
      (typeof item.reply_to !== "string" ||
        !inputs.some((input) => input.id === item.reply_to) ||
        !canReply)
    )
      throw new Error(
        "Reply target must be in this batch and replies must be permitted",
      );
    responses.push({
      text: item.text.trim(),
      ...("reply_to" in item && typeof item.reply_to === "string"
        ? { reply_to: item.reply_to }
        : {}),
    });
  }
  return responses;
}
