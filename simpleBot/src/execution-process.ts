// Fixed Deno command and lifecycle boundary. Model input never selects flags,
// inherited environment, executable, or permission grants.
interface Child {
  stdin: WritableStream<Uint8Array>;
  stdout: ReadableStream<Uint8Array>;
  stderr: ReadableStream<Uint8Array>;
  status: Promise<{ code: number; success: boolean }>;
  kill(signal: "SIGKILL"): void;
}
interface Runtime {
  execPath(): string;
  Command: new (
    executable: string,
    options: {
      args: string[];
      cwd: string;
      clearEnv: boolean;
      env: Record<string, string>;
      stdin: "piped";
      stdout: "piped";
      stderr: "piped";
    },
  ) => { spawn(): Child };
}
type StopReason = "timeout" | "cancelled" | "output_limit";
const OUTPUT_LIMIT = 32768;

export async function runExecution(
  specifier: string,
  workspace: string,
  cache: string,
  timeoutMs: number,
  signal: AbortSignal,
): Promise<unknown> {
  const runtime = (globalThis as unknown as { Deno?: Runtime }).Deno;
  if (!runtime?.Command) throw new Error("Code execution requires Deno");
  signal.throwIfAborted();
  const child = new runtime.Command(runtime.execPath(), {
    args: [
      "run",
      "--no-config",
      "--no-lock",
      "--no-npm",
      "--no-prompt",
      // cwd is fixed by the parent. Relative grants avoid CLI list parsing of
      // commas in an otherwise valid workspace path.
      "--allow-read=.",
      "--allow-write=.",
      "--allow-net",
      "--allow-import",
      "--deny-env",
      "--deny-run",
      "--deny-ffi",
      "--deny-sys",
      "--v8-flags=--max-old-space-size=128",
      "--ext=ts",
      "-",
    ],
    cwd: workspace,
    clearEnv: true,
    env: {
      DENO_DIR: cache,
      DENO_NO_UPDATE_CHECK: "1",
      NO_COLOR: "1",
      DENO_TLS_CA_STORE: "system,mozilla",
    },
    stdin: "piped",
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  let reason: StopReason | undefined;
  const kill = (): void => {
    try {
      child.kill("SIGKILL");
    } catch {
      /* Already exited. */
    }
  };
  const stop = (value: StopReason): void => {
    reason ??= value;
    kill();
  };
  const cancel = (): void => stop("cancelled");
  const timer = setTimeout(() => stop("timeout"), timeoutMs);
  signal.addEventListener("abort", cancel, { once: true });
  if (signal.aborted) cancel();
  let captured = 0;
  const collect = async (
    stream: ReadableStream<Uint8Array>,
  ): Promise<string> => {
    const reader = stream.getReader();
    const chunks: Buffer[] = [];
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        const size = Math.min(value.length, OUTPUT_LIMIT - captured);
        if (size) {
          chunks.push(Buffer.from(value.slice(0, size)));
          captured += size;
        }
        if (size < value.length) stop("output_limit");
      }
    } finally {
      reader.releaseLock();
    }
    return Buffer.concat(chunks).toString("utf8");
  };
  const writer = child.stdin.getWriter();
  const source = `const specifier = ${JSON.stringify(specifier)}; await import(specifier);`;
  const started = performance.now();
  try {
    const [status, stdout, stderr] = await Promise.all([
      child.status,
      collect(child.stdout),
      collect(child.stderr),
      writer
        .write(new TextEncoder().encode(source))
        .then(() => writer.close())
        .catch(() => {}),
    ]);
    return {
      ok: status.success && !reason,
      exitCode: status.code,
      stdout,
      stderr,
      ...(reason ? { stopped: reason } : {}),
      durationMs: Math.round(performance.now() - started),
    };
  } finally {
    clearTimeout(timer);
    signal.removeEventListener("abort", cancel);
    kill();
    await child.status.catch(() => {});
    writer.releaseLock();
  }
}
