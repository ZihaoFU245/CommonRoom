// The model owns the document structure: notes, summaries, plans, or anything
// else useful across turns. Every operation is asynchronous and conversation
// scoped. Worker bookkeeping and configuration are outside this keyspace.
import type { Kv } from "./kv.ts";
import type { FunctionTool } from "./model.ts";

export const MEMORY_TOOL: FunctionTool = {
  type: "function",
  function: {
    name: "memory",
    description:
      "Your persistent workspace for this conversation. Freely organize, read, overwrite, list and delete JSON documents. Use index as your durable entry point; it is loaded each turn. Other documents are retrieved on demand. Use version from get/set/delete for conditional writes; null creates only if absent. Omitting version overwrites deliberately. Never put secrets here.",
    parameters: {
      type: "object",
      properties: {
        action: { type: "string", enum: ["get", "set", "delete", "list"] },
        key: {
          type: "string",
          description:
            "Document name, e.g. index, topics/runtime, members/account-id",
        },
        value: { description: "Any JSON value for set" },
        version: {
          type: ["string", "null"],
          description: "Optional version for a conditional write",
        },
        prefix: {
          type: "string",
          description: "Filter document names when listing",
        },
        cursor: {
          type: "string",
          description: "Continue a previous list result",
        },
        limit: { type: "integer", minimum: 1, maximum: 50 },
      },
      required: ["action"],
      additionalProperties: false,
    },
  },
};

interface Document {
  value: unknown;
  updatedAt: number;
  sources: { id: string; authorId: string; hash: string }[];
}

export class Memory {
  private readonly kv: Kv;
  private readonly account: string;
  private readonly conversation: string;
  private readonly sources: Document["sources"];
  private readonly writable: () => boolean;
  constructor(
    kv: Kv,
    account: string,
    conversation: string,
    sources: Document["sources"] = [],
    writable: () => boolean = () => true,
  ) {
    this.kv = kv;
    this.account = account;
    this.conversation = conversation;
    this.sources = sources;
    this.writable = writable;
  }

  private key(name: string): string[] {
    return ["workspace", this.account, this.conversation, name];
  }

  /** Bounded bootstrap. Full documents remain on demand, preserving context. */
  async context(): Promise<string> {
    const [index, documents] = await Promise.all([
      this.get("index"),
      this.list("", undefined, 20),
    ]);
    return JSON.stringify({
      scope: "current conversation",
      index,
      documents,
    });
  }

  async get(name: string): Promise<unknown> {
    const entry = await this.kv.get<Document>(this.key(name));
    return { key: name, version: entry.versionstamp, ...entry.value };
  }

  async list(prefix = "", cursor?: string, limit = 20): Promise<unknown> {
    const iterator = this.kv.list<Document>(
      { prefix: this.key("").slice(0, -1) },
      { limit: 200, ...(cursor ? { cursor } : {}) },
    );
    const documents: {
      key: string;
      version: string | null;
      updatedAt: number;
    }[] = [];
    for await (const entry of iterator) {
      const name = entry.key[3];
      if (typeof name === "string" && name.startsWith(prefix) && entry.value)
        documents.push({
          key: name,
          version: entry.versionstamp,
          updatedAt: entry.value.updatedAt,
        });
      if (documents.length >= limit) break;
    }
    return { documents, cursor: iterator.cursor || null };
  }

  async execute(argumentsText: string): Promise<string> {
    try {
      const input: unknown = JSON.parse(argumentsText);
      if (typeof input !== "object" || input === null || Array.isArray(input))
        throw new Error("Expected an object");
      const args = input as Record<string, unknown>;
      if (
        Object.keys(args).some(
          (key) =>
            ![
              "action",
              "key",
              "value",
              "version",
              "prefix",
              "cursor",
              "limit",
            ].includes(key),
        )
      )
        throw new Error("Unknown memory argument");
      if (args.action === "list") {
        if (
          (args.prefix !== undefined && typeof args.prefix !== "string") ||
          (args.cursor !== undefined && typeof args.cursor !== "string") ||
          (args.limit !== undefined &&
            (!Number.isInteger(args.limit) ||
              Number(args.limit) < 1 ||
              Number(args.limit) > 50))
        )
          throw new Error("Expected prefix/cursor strings and a limit of 1-50");
        return JSON.stringify(
          await this.list(
            args.prefix as string | undefined,
            args.cursor as string | undefined,
            args.limit as number | undefined,
          ),
        );
      }
      if (
        typeof args.key !== "string" ||
        !args.key ||
        new TextEncoder().encode(args.key).length > 256 ||
        /[\u0000-\u001f]/u.test(args.key)
      )
        throw new Error(
          "Expected a document key of 1-256 UTF-8 bytes without controls",
        );
      if (args.action === "get")
        return JSON.stringify(await this.get(args.key));
      if (args.action !== "set" && args.action !== "delete")
        throw new Error("Unknown memory action");
      if (!this.writable())
        throw new Error("Conversation or turn is no longer available");
      if (
        args.version !== undefined &&
        args.version !== null &&
        typeof args.version !== "string"
      )
        throw new Error("Expected a version string or null");
      const key = this.key(args.key);
      const atomic = this.kv.atomic();
      if (args.version !== undefined)
        atomic.check({ key, versionstamp: args.version as string | null });
      if (args.action === "set") {
        if (!("value" in args)) throw new Error("set requires a JSON value");
        const document: Document = {
          value: args.value,
          updatedAt: Date.now(),
          sources: this.sources,
        };
        const bytes = new TextEncoder().encode(JSON.stringify(document)).length;
        if (
          bytes > 30000 ||
          (args.key === "index" &&
            new TextEncoder().encode(JSON.stringify(args.value)).length > 6000)
        )
          throw new Error(
            "Document exceeds 30000 bytes (index: 6000); split it into smaller documents",
          );
        atomic.set(key, document);
      } else atomic.delete(key);
      const result = await atomic.commit();
      return JSON.stringify(
        result.ok
          ? { ok: true, key: args.key, version: result.versionstamp }
          : { error: "version_conflict; read the document and retry" },
      );
    } catch (error) {
      return JSON.stringify({
        error:
          error instanceof Error ? error.message : "Memory operation failed",
      });
    }
  }
}
