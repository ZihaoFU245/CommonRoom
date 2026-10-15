import type { ChatMessage, FunctionTool } from "./model.ts";
import { createHash } from "node:crypto";

export interface TokenCount {
  tokens: number;
  source: "provider" | "provider+estimate" | "estimate";
}

/** One conversation's last measured request. Keep hashes and sizes, not text. */
export class ProviderTokens {
  tokensPerByte = 1 / 3;
  private measured:
    | {
        tokens: number;
        settings: string;
        messages: { hash: string; bytes: number }[];
      }
    | undefined;

  reset(): void {
    this.measured = undefined;
  }

  count(
    messages: ChatMessage[],
    tools?: FunctionTool[],
    reasoningEffort?: string,
  ): TokenCount {
    const previous = this.measured;
    const settings = JSON.stringify({ tools, reasoningEffort });
    const current = this.fingerprints(messages);
    if (previous?.settings === settings) {
      let common = 0;
      while (
        common < current.length &&
        common < previous.messages.length &&
        current[common]!.hash === previous.messages[common]!.hash
      )
        common++;
      if (common === current.length && common === previous.messages.length)
        return { tokens: previous.tokens, source: "provider" };
      const removedBytes = previous.messages
        .slice(common)
        .reduce((sum, message) => sum + message.bytes, 0);
      const prefixBytes = previous.messages
        .slice(0, common)
        .reduce((sum, message) => sum + message.bytes, 0);
      // Across answers the stable history prefix remains, while input metadata
      // and the question move into history. Charge new suffixes without
      // subtracting an uncertain token count for the small removed suffix.
      if (
        common > 0 &&
        (common === previous.messages.length ||
          (prefixBytes >= removedBytes &&
            removedBytes * this.tokensPerByte <= 4096))
      ) {
        const added = current.slice(common);
        return {
          tokens:
            previous.tokens +
            Math.ceil(
              added.reduce((sum, message) => sum + message.bytes, 0) *
                this.tokensPerByte,
            ) +
            8 * added.length,
          source: "provider+estimate",
        };
      }
    }
    return {
      tokens: estimateTokens({ messages, tools }, this.tokensPerByte),
      source: "estimate",
    };
  }

  observe(
    messages: ChatMessage[],
    tools: FunctionTool[] | undefined,
    reasoningEffort: string | undefined,
    promptTokens: number,
  ): void {
    if (!Number.isSafeInteger(promptTokens) || promptTokens < 0) return;
    const bytes = Buffer.byteLength(
      JSON.stringify({ messages, tools }),
      "utf8",
    );
    if (bytes > 0)
      this.tokensPerByte = Math.max(
        this.tokensPerByte,
        (1.1 * promptTokens) / bytes,
      );
    this.measured = {
      tokens: promptTokens,
      settings: JSON.stringify({ tools, reasoningEffort }),
      messages: this.fingerprints(messages),
    };
  }

  private fingerprints(
    messages: ChatMessage[],
  ): { hash: string; bytes: number }[] {
    return messages.map((message) => {
      const text = JSON.stringify(message);
      return {
        hash: createHash("sha256").update(text).digest("hex"),
        bytes: Buffer.byteLength(text, "utf8"),
      };
    });
  }
}

/** Lightweight estimate, calibrated upward from provider usage by ModelClient.
 * UTF-8 bytes account for non-ASCII text. This is not a provider tokenizer. */
export function estimateTokens(value: unknown, tokensPerByte = 1 / 3): number {
  return (
    Math.ceil(
      Buffer.byteLength(JSON.stringify(value), "utf8") * tokensPerByte,
    ) + 32
  );
}

/** Remove oldest history first; preserve system instructions, current input and
 * the tool-call/result protocol. No model call is needed for compaction. */
export function compactConversation(
  messages: ChatMessage[],
  tools: FunctionTool[] | undefined,
  target: number,
  tokensPerByte: number,
): ChatMessage[] {
  const result = messages.map((message) => ({ ...message }));
  while (estimateTokens({ messages: result, tools }, tokensPerByte) > target) {
    const index = result.findIndex(
      (message) =>
        message.role === "user" &&
        message.content.startsWith(
          "Conversation data [sequence,id,sender,text,offset,author_id]:\n",
        ),
    );
    if (index < 0) break;
    const message = result[index]!;
    const prefix =
      "Conversation data [sequence,id,sender,text,offset,author_id]:\n";
    let rows: unknown[] = [];
    try {
      const page = JSON.parse(message.content.slice(prefix.length)) as {
        messages?: unknown;
      };
      if (Array.isArray(page.messages)) rows = page.messages;
    } catch {
      /* Remove an unavailable/malformed history chunk. */
    }
    if (rows.length < 2) result.splice(index, 1);
    else
      message.content =
        prefix +
        JSON.stringify({
          messages: rows.slice(Math.ceil(rows.length / 2)),
          truncated: true,
        });
  }
  // Large retrieval results can themselves exceed the remaining context.
  // Keep the call IDs and explicit truncation; never rerun effects to compact.
  for (const message of result) {
    if (estimateTokens({ messages: result, tools }, tokensPerByte) <= target)
      break;
    if (message.role === "tool" && message.content.length > 512)
      message.content =
        message.content.slice(0, 512) +
        "\n[Result truncated to fit context budget.]";
  }
  return result;
}
