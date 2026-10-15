// The parent chooses the executable and permissions. Generated code is loaded
// dynamically by a trusted bootstrap, avoiding the initial module graph's
// exemption from filesystem permissions. Each call owns and reaps one process.
import { mkdir, mkdtemp, lstat, readdir, realpath, rm } from "node:fs/promises";
import { dirname, join, resolve, relative, isAbsolute } from "node:path";
import { pathToFileURL } from "node:url";
import { runExecution } from "./execution-process.ts";
import type { FunctionTool } from "./model.ts";

export const EXECUTION_TOOL: FunctionTool = {
  type: "function",
  function: {
    name: "execute_code",
    description:
      "Run JavaScript/TypeScript in a fresh Deno subprocess. Supply code OR a saved workspace-relative .js/.mjs/.ts/.mts file. Top-level await, built-in APIs, HTTPS imports, and network requests are available. cwd is the persistent shared data/workspace folder, with read/write access to its entire tree. Files are shared across all this bot's rooms and DMs. Environment, worker state/configuration, subprocesses and FFI are unavailable. Print results with console.log; globals reset each call. Save reusable scripts as files or in memory. No automatic retries: network and file effects may have happened even on failure or timeout.",
    parameters: {
      type: "object",
      properties: {
        code: {
          type: "string",
          description: "JS/TS source; use exactly one of code or file",
        },
        file: {
          type: "string",
          description:
            "Saved script path relative to workspace; use exactly one of code or file",
        },
        timeoutMs: {
          type: "integer",
          minimum: 100,
          maximum: 30000,
          description:
            "Wall-clock limit; defaults to 10000 ms, also bounded by the agent turn",
        },
      },
      additionalProperties: false,
    },
  },
};

interface Request {
  code?: string;
  file?: string;
  timeoutMs: number;
}

function request(raw: string): Request {
  const value: unknown = JSON.parse(raw);
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new Error("Expected an object");
  const input = value as Record<string, unknown>;
  if (
    Object.keys(input).some(
      (key) => !["code", "file", "timeoutMs"].includes(key),
    ) ||
    "code" in input === "file" in input
  )
    throw new Error(
      "Supply exactly one of code or file, and optional timeoutMs",
    );
  const timeoutMs = input.timeoutMs ?? 10000;
  if (
    typeof timeoutMs !== "number" ||
    !Number.isInteger(timeoutMs) ||
    timeoutMs < 100 ||
    timeoutMs > 30000
  )
    throw new Error("timeoutMs must be an integer from 100 to 30000");
  if ("code" in input) {
    if (
      typeof input.code !== "string" ||
      !input.code.trim() ||
      Buffer.byteLength(input.code) > 48000
    )
      throw new Error("code must contain 1-48000 UTF-8 bytes of JS/TS");
    return { code: input.code, timeoutMs };
  }
  if (
    typeof input.file !== "string" ||
    !input.file ||
    input.file.length > 1024 ||
    isAbsolute(input.file) ||
    !/\.(?:[cm]?js|[cm]?ts)$/u.test(input.file)
  )
    throw new Error("file must be a workspace-relative JS/TS path");
  return { file: input.file, timeoutMs };
}

/** Existing symlinks inherit the link location's Deno permissions, so refuse
 * them before running code. Scoped Deno permissions prevent creating new ones. */
async function checkTree(
  directory: string,
  signal: AbortSignal,
): Promise<void> {
  signal.throwIfAborted();
  if ((await lstat(directory)).isSymbolicLink())
    throw new Error("Execution workspace must not contain symbolic links");
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    signal.throwIfAborted();
    if (entry.isSymbolicLink())
      throw new Error("Execution workspace must not contain symbolic links");
    if (entry.isDirectory())
      await checkTree(join(directory, entry.name), signal);
  }
}

export class Execution {
  readonly workspace: string;
  private readonly pending = new Set<Promise<string>>();
  constructor(workspace: string) {
    this.workspace = resolve(workspace);
  }

  execute(raw: string, signal: AbortSignal): Promise<string> {
    const work = this.perform(raw, signal);
    this.pending.add(work);
    work.then(
      () => this.pending.delete(work),
      () => this.pending.delete(work),
    );
    return work;
  }

  async settle(): Promise<void> {
    await Promise.allSettled([...this.pending]);
  }

  private async perform(raw: string, signal: AbortSignal): Promise<string> {
    try {
      const input = request(raw);
      signal.throwIfAborted();
      await mkdir(this.workspace, { recursive: true, mode: 0o700 });
      await checkTree(this.workspace, signal);
      const workspace = await realpath(this.workspace);
      let specifier: string;
      if (input.file !== undefined) {
        const path = await realpath(resolve(this.workspace, input.file));
        const child = relative(workspace, path);
        if (
          !child ||
          child === ".." ||
          child.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) ||
          isAbsolute(child)
        )
          throw new Error("Script must be inside the execution workspace");
        specifier = pathToFileURL(path).href;
      } else
        specifier = `data:application/typescript;base64,${Buffer.from(input.code ?? "").toString("base64")}`;
      signal.throwIfAborted();
      const cache = await mkdtemp(join(dirname(this.workspace), ".execution-"));
      try {
        return JSON.stringify(
          await runExecution(
            specifier,
            workspace,
            cache,
            input.timeoutMs,
            signal,
          ),
        );
      } finally {
        await rm(cache, { recursive: true, force: true });
      }
    } catch (error) {
      return JSON.stringify({
        error: signal.aborted
          ? "Execution cancelled"
          : error instanceof Error
            ? error.message
            : "Code execution failed",
      });
    }
  }
}
