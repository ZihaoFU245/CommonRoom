import type { ToolCall } from "./model.ts";

/** Response structure only: never log arguments, content or reasoning. */
export function tickResponseShape(
  text: string,
  calls: ToolCall[],
  available: string[],
): string {
  const known = new Set(["finish_tick", ...available]);
  return JSON.stringify({
    text_chars: text.length,
    tool_calls: calls.length,
    calls: calls.map((call) => {
      let value: unknown;
      let kind = "invalid_json";
      try {
        value = JSON.parse(call.function.arguments);
        kind =
          value === null
            ? "null"
            : Array.isArray(value)
              ? "array"
              : typeof value;
      } catch {
        /* Deliberately omit parser errors containing response text. */
      }
      const decision =
        call.function.name === "finish_tick" && kind === "object"
          ? (value as Record<string, unknown>)
          : undefined;
      const next = decision?.next;
      const nextKind =
        next && typeof next === "object" && "kind" in next
          ? next.kind
          : undefined;
      return {
        tool: known.has(call.function.name)
          ? call.function.name
          : "unavailable",
        arguments_chars: call.function.arguments.length,
        arguments_json: kind,
        ...(decision
          ? {
              messages: Array.isArray(decision.messages)
                ? decision.messages.length
                : "missing_or_invalid",
              actions: Array.isArray(decision.actions)
                ? decision.actions.length
                : "missing_or_invalid",
              next:
                typeof nextKind === "string" &&
                ["wait", "continue", "after", "on_jobs"].includes(nextKind)
                  ? nextKind
                  : "missing_or_invalid",
              unknown_fields: Object.keys(decision).filter(
                (key) => !["messages", "actions", "next"].includes(key),
              ).length,
            }
          : {}),
      };
    }),
  });
}
