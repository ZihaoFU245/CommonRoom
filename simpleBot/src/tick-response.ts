import { isDeepStrictEqual } from "node:util";
import type { FunctionTool, ToolCall } from "./model.ts";
import { pruneSchema, tickDecision, type TickDecision } from "./tick.ts";

type Parsed = { ok: true; value: unknown } | { ok: false };

/**
 * Parse tool arguments, tolerating the two shapes gateways produce in practice:
 * a fenced or prose-wrapped object, and a double-encoded JSON string. Anything
 * else still fails; the caller never sees parser text, which can quote values.
 */
function parseJson(text: string): Parsed {
  const trimmed = text.trim();
  const candidates = [text, trimmed];
  const fenced = /```(?:json)?\s*([\s\S]*?)\s*```/u.exec(trimmed);
  if (fenced?.[1]) candidates.push(fenced[1]);
  const object = trimmed.indexOf("{");
  const objectEnd = trimmed.lastIndexOf("}");
  if (object >= 0 && objectEnd > object)
    candidates.push(trimmed.slice(object, objectEnd + 1));
  const array = trimmed.indexOf("[");
  const arrayEnd = trimmed.lastIndexOf("]");
  if (array >= 0 && arrayEnd > array)
    candidates.push(trimmed.slice(array, arrayEnd + 1));
  for (const candidate of candidates)
    try {
      return { ok: true, value: JSON.parse(candidate) };
    } catch {
      // Try the next shape.
    }
  return { ok: false };
}

function argumentsObject(call: ToolCall, label: string): unknown {
  const parsed = parseJson(call.function.arguments);
  if (!parsed.ok) throw new Error(`${label}: expected valid JSON arguments`);
  // A provider may encode the argument object as a JSON string.
  if (typeof parsed.value === "string") {
    const nested = parseJson(parsed.value);
    if (nested.ok && typeof nested.value !== "string") return nested.value;
  }
  return parsed.value;
}

/** Normalize native calls and an optional envelope before accepting any effects. */
export function tickResponse(
  calls: ToolCall[],
  tools: FunctionTool[],
): { decision: TickDecision; coalesced: number; normalized: string[] } {
  const normalized: string[] = [];
  const envelopes = calls.filter(
    (call) => call.function.name === "finish_tick",
  );
  if (envelopes.length > 1)
    throw new Error("Expected at most one finish_tick decision");
  const envelope = envelopes[0];
  const decision: TickDecision = envelope
    ? tickDecision(argumentsObject(envelope, "finish_tick"), tools, normalized)
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
    const tool = tools.find(
      (tool) => tool.function.name === call.function.name,
    );
    if (!tool) throw new Error(`native_calls[${index}]: unavailable tool`);
    // Compare the same normalized shapes: envelope arguments have already been
    // pruned by tickDecision. Otherwise a stray native field duplicates an effect.
    const args = pruneSchema(
      argumentsObject(call, `native_calls[${index}]`),
      tool.function.parameters,
      tool.function.name,
      normalized,
    );
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
  return {
    decision: tickDecision(decision, tools, normalized),
    coalesced,
    normalized,
  };
}
