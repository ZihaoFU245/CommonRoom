import type { FunctionTool } from "./model.ts";

export type NextTick =
  | { kind: "wait" }
  | { kind: "continue" }
  | { kind: "after"; delayMs: number }
  | { kind: "on_jobs"; jobs: string[]; mode: "any" | "all" };

export interface TickDecision {
  messages: { text: string; replyTo?: string }[];
  actions: { id: string; tool: string; arguments: unknown }[];
  next: NextTick;
}

export interface TickJob {
  id: string;
  tool: string;
  root?: string;
  sources?: string[];
  arguments: string;
  status: "queued" | "running" | "completed" | "failed" | "interrupted";
  created: number;
  result?: string;
  observed?: boolean;
}

/** One record owns the atomic decision, job intents and one-shot continuation. */
export interface TickSession {
  root: string;
  generation: number;
  tick: number;
  calls: number;
  tokens?: number;
  started: number;
  phase: "ready" | "calling" | "idle" | "failed";
  next: NextTick;
  dueAt?: number;
  jobs: Record<string, TickJob>;
  sources: { input: string; hash: string; sequence: number }[];
  trace: { tick: number; messages: string[]; actions: string[] }[];
  error?: string;
  retries?: number;
  retryInputs?: string[];
}

export function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Validate the supported JSON Schema subset before any proposed effect starts. */
export function validateArguments(
  value: unknown,
  schema: Record<string, unknown>,
): void {
  if (Array.isArray(schema.enum) && !schema.enum.includes(value))
    throw new Error("Invalid enum value");
  if (schema.type === "object") {
    if (!object(value)) throw new Error("Expected argument object");
    const properties = object(schema.properties) ? schema.properties : {};
    if (
      Array.isArray(schema.required) &&
      schema.required.some((key) => typeof key === "string" && !(key in value))
    )
      throw new Error("Missing required argument");
    for (const [key, item] of Object.entries(value)) {
      const child = properties[key];
      if (object(child)) validateArguments(item, child);
      else if (schema.additionalProperties === false)
        throw new Error("Unknown argument");
    }
  } else if (schema.type === "array") {
    if (!Array.isArray(value)) throw new Error("Expected array");
    if (typeof schema.maxItems === "number" && value.length > schema.maxItems)
      throw new Error("Too many items");
    if (typeof schema.minItems === "number" && value.length < schema.minItems)
      throw new Error("Too few items");
    if (object(schema.items))
      for (const item of value) validateArguments(item, schema.items);
  } else if (schema.type === "string") {
    if (typeof value !== "string") throw new Error("Expected string");
    const length = Array.from(value).length;
    if (
      (typeof schema.minLength === "number" && length < schema.minLength) ||
      (typeof schema.maxLength === "number" && length > schema.maxLength)
    )
      throw new Error("Invalid string length");
  } else if (schema.type === "number" || schema.type === "integer") {
    if (
      typeof value !== "number" ||
      !Number.isFinite(value) ||
      (schema.type === "integer" && !Number.isSafeInteger(value))
    )
      throw new Error("Expected bounded number");
    if (
      (typeof schema.minimum === "number" && value < schema.minimum) ||
      (typeof schema.maximum === "number" && value > schema.maximum)
    )
      throw new Error("Argument outside bounds");
  } else if (schema.type === "boolean" && typeof value !== "boolean")
    throw new Error("Expected boolean");
}

const nextSchema = {
  type: "object",
  properties: {
    kind: { type: "string", enum: ["wait", "continue", "after", "on_jobs"] },
    delayMs: { type: "integer", minimum: 1, maximum: 86400000 },
    jobs: {
      type: "array",
      minItems: 1,
      maxItems: 128,
      items: { type: "string" },
    },
    mode: { type: "string", enum: ["any", "all"] },
  },
  required: ["kind"],
  additionalProperties: false,
};

export function finishTickTool(tools: FunctionTool[]): FunctionTool {
  return {
    type: "function",
    function: {
      name: "finish_tick",
      description:
        "Commit this tick: messages (or [] for silence), independent tool jobs, and a one-shot next tick. Use on_jobs to yield for results; continue only for useful independent work. Jobs survive this tick. New eligible messages can always wake you. Never repeat an action to check status.",
      parameters: {
        type: "object",
        properties: {
          messages: {
            type: "array",
            maxItems: 16,
            items: {
              type: "object",
              properties: {
                text: { type: "string", minLength: 1, maxLength: 262144 },
                replyTo: { type: "string" },
              },
              required: ["text"],
              additionalProperties: false,
            },
          },
          actions: {
            type: "array",
            maxItems: tools.length ? 16 : 0,
            items: tools.length
              ? {
                  anyOf: tools.map((tool) => ({
                    type: "object",
                    properties: {
                      id: { type: "string", minLength: 1, maxLength: 64 },
                      tool: { type: "string", enum: [tool.function.name] },
                      arguments: tool.function.parameters,
                    },
                    required: ["id", "tool", "arguments"],
                    additionalProperties: false,
                  })),
                }
              : { type: "object" },
          },
          next: nextSchema,
        },
        required: ["messages", "actions", "next"],
        additionalProperties: false,
      },
    },
  };
}

export function nextTick(value: unknown): NextTick {
  validateArguments(value, nextSchema);
  if (!object(value)) throw new Error("Invalid continuation");
  if (value.kind === "wait" || value.kind === "continue") {
    if (Object.keys(value).length !== 1)
      throw new Error("Unexpected continuation arguments");
    return { kind: value.kind };
  }
  if (
    value.kind === "after" &&
    typeof value.delayMs === "number" &&
    Object.keys(value).length === 2
  )
    return { kind: "after", delayMs: value.delayMs };
  if (
    value.kind === "on_jobs" &&
    Array.isArray(value.jobs) &&
    value.jobs.length &&
    value.jobs.every((id) => typeof id === "string") &&
    new Set(value.jobs).size === value.jobs.length &&
    (value.mode === "any" || value.mode === "all") &&
    Object.keys(value).length === 3
  )
    return { kind: "on_jobs", jobs: value.jobs as string[], mode: value.mode };
  throw new Error("Invalid continuation arguments");
}

export function tickDecision(
  value: unknown,
  tools: FunctionTool[],
): TickDecision {
  if (
    !object(value) ||
    Object.keys(value).some(
      (key) => !["messages", "actions", "next"].includes(key),
    ) ||
    !Array.isArray(value.messages) ||
    !Array.isArray(value.actions) ||
    value.messages.length > 16 ||
    value.actions.length > 16
  )
    throw new Error("Expected messages, actions and next");
  const messages = value.messages.map((item: unknown) => {
    if (
      !object(item) ||
      typeof item.text !== "string" ||
      !item.text.trim() ||
      Array.from(item.text).length > 262144 ||
      (item.replyTo !== undefined && typeof item.replyTo !== "string") ||
      Object.keys(item).some((key) => !["text", "replyTo"].includes(key))
    )
      throw new Error("Invalid tick message");
    return {
      text: item.text,
      ...(typeof item.replyTo === "string" ? { replyTo: item.replyTo } : {}),
    };
  });
  const ids = new Set<string>();
  const actions = value.actions.map((item: unknown) => {
    if (
      !object(item) ||
      typeof item.id !== "string" ||
      !/^[A-Za-z0-9._-]{1,64}$/.test(item.id) ||
      ids.has(item.id) ||
      typeof item.tool !== "string" ||
      Object.keys(item).some(
        (key) => !["id", "tool", "arguments"].includes(key),
      )
    )
      throw new Error("Invalid action identity");
    const tool = tools.find((tool) => tool.function.name === item.tool);
    if (!tool) throw new Error("Unavailable tool");
    validateArguments(item.arguments, tool.function.parameters);
    if (JSON.stringify(item.arguments)?.length > 65536)
      throw new Error("Oversized action arguments");
    if (item.tool === "memory" && object(item.arguments)) {
      const args = item.arguments;
      if (
        args.action !== "list" &&
        (typeof args.key !== "string" ||
          !args.key ||
          Buffer.byteLength(args.key) > 256 ||
          /[\u0000-\u001f]/u.test(args.key))
      )
        throw new Error("Invalid memory key");
      if (args.action === "set" && !("value" in args))
        throw new Error("Memory set requires a value");
      if (
        args.version !== undefined &&
        args.version !== null &&
        typeof args.version !== "string"
      )
        throw new Error("Invalid memory version");
    }
    if (
      item.tool === "execute_code" &&
      object(item.arguments) &&
      "code" in item.arguments === "file" in item.arguments
    )
      throw new Error("Supply code or file");
    ids.add(item.id);
    return { id: item.id, tool: item.tool, arguments: item.arguments };
  });
  return { messages, actions, next: nextTick(value.next) };
}

export function jobTerminal(job: TickJob): boolean {
  return !["queued", "running"].includes(job.status);
}

export function sessionReady(session: TickSession, now = Date.now()): boolean {
  if (session.phase === "failed" || session.phase === "calling") return false;
  const next = session.next;
  if (next.kind === "continue") return true;
  if (next.kind === "after") return (session.dueAt ?? Infinity) <= now;
  if (next.kind === "on_jobs") {
    const states = next.jobs
      .map((id) => session.jobs[id])
      .map((job) => job !== undefined && jobTerminal(job));
    return next.mode === "all" ? states.every(Boolean) : states.some(Boolean);
  }
  return false;
}
