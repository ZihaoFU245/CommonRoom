import { spawn } from "node:child_process";

export interface ShellResult {
  ok: boolean;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  stopped?: "timeout" | "cancelled" | "output_limit";
  error?: string;
  signal?: string;
  durationMs: number;
}

export interface ShellInvocation {
  done: Promise<ShellResult>;
  initial: Promise<{ running: true } | { running: false; result: ShellResult }>;
}

/** Command supervision only. No filesystem, environment or executable policy. */
export function runShell(
  command: string,
  cwd: string,
  timeoutMs: number,
  waitMs: number,
  signal: AbortSignal,
): ShellInvocation {
  signal.throwIfAborted();
  const started = performance.now();
  const child = spawn("/bin/sh", ["-c", command], {
    cwd,
    env: process.env,
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let reason: ShellResult["stopped"];
  let error: string | undefined;
  let captured = 0;
  const stdout: Buffer[] = [];
  const stderr: Buffer[] = [];
  const kill = () => {
    if (!child.pid) return;
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      try {
        child.kill("SIGKILL");
      } catch {
        /* Already reaped. */
      }
    }
  };
  const stop = (why: NonNullable<ShellResult["stopped"]>) => {
    reason ??= why;
    kill();
    // Escaped/detached descendants may retain descriptors. Do not let those
    // descriptors hold the worker open; external sandbox supervision owns them.
    child.stdout.destroy();
    child.stderr.destroy();
  };
  const collect = (chunks: Buffer[], data: Buffer | string) => {
    const bytes = Buffer.isBuffer(data) ? data : Buffer.from(data);
    const size = Math.min(bytes.length, 32768 - captured);
    if (size) {
      chunks.push(Buffer.from(bytes.subarray(0, size)));
      captured += size;
    }
    if (size < bytes.length) stop("output_limit");
  };
  child.stdout.on("data", (data: Buffer | string) => collect(stdout, data));
  child.stderr.on("data", (data: Buffer | string) => collect(stderr, data));
  child.on("error", (value) => {
    error = value.message;
  });
  const cancel = () => stop("cancelled");
  const timeout = setTimeout(() => stop("timeout"), timeoutMs);
  signal.addEventListener("abort", cancel, { once: true });
  const done = new Promise<ShellResult>((resolve) => {
    child.once("close", (exitCode, exitSignal) => {
      clearTimeout(timeout);
      signal.removeEventListener("abort", cancel);
      kill();
      resolve({
        ok: exitCode === 0 && !reason && !error,
        exitCode,
        stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: Buffer.concat(stderr).toString("utf8"),
        ...(reason ? { stopped: reason } : {}),
        ...(exitSignal ? { signal: exitSignal } : {}),
        ...(error ? { error } : {}),
        durationMs: Math.round(performance.now() - started),
      });
    });
  });
  if (signal.aborted) cancel();
  const initial = new Promise<
    { running: true } | { running: false; result: ShellResult }
  >((resolve) => {
    const grace = setTimeout(() => resolve({ running: true }), waitMs);
    done.then((result) => {
      clearTimeout(grace);
      resolve({ running: false, result });
    });
  });
  return { done, initial };
}
