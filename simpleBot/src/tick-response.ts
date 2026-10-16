import { isDeepStrictEqual } from "node:util";
import type { FunctionTool, ToolCall } from "./model.ts";
import { tickDecision, type TickDecision } from "./tick.ts";

function argumentsObject(call: ToolCall, label: string): unknown {
  try {
    return JSON.parse(call.function.arguments);
  } catch {
    // Parser errors can include private argument values.
    throw new Error(`${label}: expected valid JSON arguments`);
  }
}

/** Normalize native calls and an optional envelope before accepting any effects. */
export function tickResponse(
  calls: ToolCall[],
  tools: FunctionTool[],
): { decision: TickDecision; coalesced: number } {
  const envelopes = calls.filter(
    (call) => call.function.name === "finish_tick",
  );
  if (envelopes.length > 1)
    throw new Error("Expected at most one finish_tick decision");
  const envelope = envelopes[0];
  const decision: TickDecision = envelope
    ? tickDecision(argumentsObject(envelope, "finish_tick"), tools)
    : { messages: [], actions: [], next: { kind: "wait" } };
  const natives = calls.filter((call) => call.function.name !== "finish_tick");
  const matched = new Set<string>();
  const aliases = new Map<string, string>();
  const nativeIds: string[] = [];
  const callIds = new Set<string>();
  let coalesced = 0;
  for (const [index, call] of natives.entries()) {
    if (callIds.has(call.id))
      throw new Error(`native_calls[${index}]: duplicate tool call ID`);
    callIds.add(call.id);
    if (!tools.some((tool) => tool.function.name === call.function.name))
      throw new Error(`native_calls[${index}]: unavailable tool`);
    const args = argumentsObject(call, `native_calls[${index}]`);
    // Providers sometimes encode the same action both ways. Match one-to-one;
    // distinct native calls are still distinct intents, even with equal args.
    const existing = decision.actions.find(
      (action) =>
        !matched.has(action.id) &&
        action.tool === call.function.name &&
        isDeepStrictEqual(action.arguments, args),
    );
    let id: string;
    if (existing) {
      id = existing.id;
      matched.add(id);
      coalesced++;
    } else {
      let suffix = index + 1;
      id = `native-${suffix}`;
      while (decision.actions.some((action) => action.id === id))
        id = `native-${++suffix}`;
      decision.actions.push({ id, tool: call.function.name, arguments: args });
      matched.add(id);
    }
    // A reference must not ambiguously name an envelope action and a native call.
    if (
      call.id !== id &&
      decision.actions.some((action) => action.id === call.id)
    )
      throw new Error(`native_calls[${index}]: ambiguous continuation ID`);
    aliases.set(call.id, id);
    nativeIds.push(id);
  }
  if (!envelope && nativeIds.length)
    decision.next = { kind: "on_jobs", jobs: nativeIds, mode: "all" };
  else if (decision.next.kind === "on_jobs")
    decision.next.jobs = [
      ...new Set(decision.next.jobs.map((id) => aliases.get(id) ?? id)),
    ];
  return { decision: tickDecision(decision, tools), coalesced };
}
