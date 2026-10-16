import { mkdir } from "node:fs/promises";
import { homedir } from "node:os";
import { resolve } from "node:path";
import type { FunctionTool } from "./model.ts";
import { object } from "./tick.ts";
import { runShell } from "./shell-process.ts";

export const SHELL_TOOL: FunctionTool = {
  type: "function",
  function: {
    name: "run_shell",
    description:
      "Run a command using /bin/sh -c with the deployment's environment and access. The initial cwd is the agent workspace; cwd may be absolute, workspace-relative or ~. Shell operators, pipes, redirects, cd and available binaries work normally. Each command uses a fresh shell; files persist. waitMs is the quick-result grace period; after it the command remains a background job. timeoutMs kills its process group. Use next.on_jobs for results, never repeat effects to poll. No automatic replay after interruption.",
    parameters: {
      type: "object",
      properties: {
        command: { type: "string", minLength: 1, maxLength: 32768 },
        cwd: { type: "string", minLength: 1, maxLength: 4096 },
        waitMs: {
          type: "integer",
          minimum: 0,
          maximum: 30000,
          description:
            "Quick-result grace, default 1000 ms; does not occupy a model slot",
        },
        timeoutMs: {
          type: "integer",
          minimum: 100,
          maximum: 600000,
          description:
            "Command lifetime, default 60000 ms, bounded by the episode deadline",
        },
      },
      required: ["command"],
      additionalProperties: false,
    },
  },
};

export interface ShellRequest {
  command: string;
  cwd?: string;
  waitMs: number;
  timeoutMs: number;
}

export function shellRequest(raw: string): ShellRequest {
  const input: unknown = JSON.parse(raw);
  if (
    !object(input) ||
    typeof input.command !== "string" ||
    !input.command.trim() ||
    input.command.length > 32768 ||
    input.command.includes("\0") ||
    Object.keys(input).some(
      (key) => !["command", "cwd", "waitMs", "timeoutMs"].includes(key),
    ) ||
    (input.cwd !== undefined &&
      (typeof input.cwd !== "string" ||
        !input.cwd ||
        input.cwd.length > 4096 ||
        input.cwd.includes("\0")))
  )
    throw new Error("Invalid shell arguments");
  const waitMs = input.waitMs ?? 1000;
  const timeoutMs = input.timeoutMs ?? 60000;
  if (
    typeof waitMs !== "number" ||
    !Number.isSafeInteger(waitMs) ||
    waitMs < 0 ||
    waitMs > 30000 ||
    typeof timeoutMs !== "number" ||
    !Number.isSafeInteger(timeoutMs) ||
    timeoutMs < 100 ||
    timeoutMs > 600000
  )
    throw new Error("Invalid shell wait or timeout bounds");
  return {
    command: input.command,
    ...(typeof input.cwd === "string" ? { cwd: input.cwd } : {}),
    waitMs,
    timeoutMs,
  };
}

export class Shell {
  readonly enabled: boolean;
  readonly workspace: string;
  private readonly pending = new Set<Promise<string>>();
  constructor(workspace: string, enabled: boolean) {
    this.workspace = workspace;
    this.enabled = enabled;
  }

  execute(
    raw: string,
    signal: AbortSignal,
    onYield?: () => Promise<void>,
  ): Promise<string> {
    if (!this.enabled)
      return Promise.reject(
        new Error("Shell access requires --with-shell at startup"),
      );
    const work = this.perform(raw, signal, onYield);
    this.pending.add(work);
    work.then(
      () => this.pending.delete(work),
      () => this.pending.delete(work),
    );
    return work;
  }

  async settle(): Promise<void> {
    await Promise.allSettled(this.pending);
  }

  private async perform(
    raw: string,
    signal: AbortSignal,
    onYield?: () => Promise<void>,
  ): Promise<string> {
    const input = shellRequest(raw);
    signal.throwIfAborted();
    await mkdir(this.workspace, { recursive: true, mode: 0o700 });
    const directory =
      input.cwd === "~"
        ? homedir()
        : input.cwd?.startsWith("~/")
          ? resolve(homedir(), input.cwd.slice(2))
          : resolve(this.workspace, input.cwd ?? ".");
    const controller = new AbortController();
    const invocation = runShell(
      input.command,
      directory,
      input.timeoutMs,
      input.waitMs,
      AbortSignal.any([signal, controller.signal]),
    );
    try {
      const initial = await invocation.initial;
      if (initial.running) await onYield?.();
      return JSON.stringify(await invocation.done);
    } finally {
      controller.abort();
      await invocation.done;
    }
  }
}
