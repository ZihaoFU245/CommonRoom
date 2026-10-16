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
  waitExpired?: boolean;
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
  path = "arguments",
): void {
  const invalid = (reason: string): never => {
    throw new Error(`${path}: ${reason}`);
  };
  if (Array.isArray(schema.enum) && !schema.enum.includes(value))
    invalid("Invalid enum value; use an advertised enum value");
  if (schema.type === "object") {
    if (!object(value)) return invalid("Expected argument object");
    const properties = object(schema.properties) ? schema.properties : {};
    const missing = Array.isArray(schema.required)
      ? schema.required.filter(
          (key): key is string =>
            typeof key === "string" && !Object.hasOwn(value, key),
        )
      : [];
    if (missing.length)
      invalid(
        `Missing required argument; required fields: ${JSON.stringify(missing)}`,
      );
    for (const [key, item] of Object.entries(value)) {
      const child = Object.hasOwn(properties, key)
        ? properties[key]
        : undefined;
      if (object(child)) validateArguments(item, child, `${path}.${key}`);
      else if (schema.additionalProperties === false)
        // Report the host-owned contract, never arbitrary model field names/values.
        invalid(
          `Unknown argument; allowed fields: ${JSON.stringify(Object.keys(properties))}`,
        );
    }
  } else if (schema.type === "array") {
    if (!Array.isArray(value)) return invalid("Expected array");
    if (typeof schema.maxItems === "number" && value.length > schema.maxItems)
      invalid(`Too many items; maximum ${schema.maxItems}`);
    if (typeof schema.minItems === "number" && value.length < schema.minItems)
      invalid(`Too few items; minimum ${schema.minItems}`);
    if (object(schema.items))
      for (const [index, item] of value.entries())
        validateArguments(item, schema.items, `${path}[${index}]`);
  } else if (schema.type === "string") {
    if (typeof value !== "string") return invalid("Expected string");
    const length = Array.from(value).length;
    if (
      (typeof schema.minLength === "number" && length < schema.minLength) ||
      (typeof schema.maxLength === "number" && length > schema.maxLength)
    )
      invalid(
        `Invalid string length; minimum ${schema.minLength ?? 0}, maximum ${schema.maxLength ?? "unbounded"}`,
      );
  } else if (schema.type === "number" || schema.type === "integer") {
    if (
      typeof value !== "number" ||
      !Number.isFinite(value) ||
      (schema.type === "integer" && !Number.isSafeInteger(value))
    )
      return invalid(
        `Expected ${schema.type === "integer" ? "integer" : "finite number"}`,
      );
    if (
      (typeof schema.minimum === "number" && value < schema.minimum) ||
      (typeof schema.maximum === "number" && value > schema.maximum)
    )
      invalid(
        `Argument outside bounds; minimum ${schema.minimum ?? "unbounded"}, maximum ${schema.maximum ?? "unbounded"}`,
      );
  } else if (schema.type === "boolean" && typeof value !== "boolean")
    invalid("Expected boolean");
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

export function finishTickTool(
  tools: FunctionTool[],
  replyTargets?: string[],
  maxReplyMessages = 16,
): FunctionTool {
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
            maxItems: Math.min(16, Math.max(0, maxReplyMessages)),
            items: {
              type: "object",
              properties: {
                text: { type: "string", minLength: 1, maxLength: 262144 },
                ...(replyTargets?.length === 0
                  ? {}
                  : {
                      replyTo: {
                        type: "string",
                        ...(replyTargets ? { enum: replyTargets } : {}),
                      },
                    }),
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
                      id: {
                        type: "string",
                        minLength: 1,
                        maxLength: 64,
                        description:
                          "A unique action ID within this tick. Use this ID in next.jobs.",
                      },
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
        required: ["next"],
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
  // Omitted output/job lists unambiguously mean no messages/jobs. Invalid
  // explicit values still fail; continuation always remains mandatory.
  if (object(value)) value = { messages: [], actions: [], ...value };
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
  const actions = value.actions.map((item: unknown, index: number) => {
    if (
      !object(item) ||
      typeof item.id !== "string" ||
      Object.keys(item).some(
        (key) => !["id", "tool", "arguments"].includes(key),
      )
    )
      throw new Error(
        `actions[${index}]: expected id, tool and arguments only`,
      );
    if (Array.from(item.id).length < 1 || Array.from(item.id).length > 64)
      throw new Error(`actions[${index}].id: expected 1 to 64 characters`);
    if (ids.has(item.id))
      throw new Error(
        `actions[${index}].id: duplicate action ID; use a unique ID within this tick`,
      );
    if (typeof item.tool !== "string")
      throw new Error(`actions[${index}].tool: expected a tool name`);
    const tool = tools.find((tool) => tool.function.name === item.tool);
    if (!tool) throw new Error(`actions[${index}].tool: unavailable tool`);
    validateArguments(
      item.arguments,
      tool.function.parameters,
      `actions[${index}].arguments (${tool.function.name})`,
    );
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
    const jobs = next.jobs.map((id) =>
      Object.hasOwn(session.jobs, id) ? session.jobs[id] : undefined,
    );
    const fresh = jobs.some(
      (job) => job !== undefined && jobTerminal(job) && !job.observed,
    );
    return (
      fresh &&
      (next.mode === "any" ||
        jobs.every((job) => job !== undefined && jobTerminal(job)))
    );
  }
  return false;
}
