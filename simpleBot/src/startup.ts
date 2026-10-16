import { resolve } from "node:path";

export interface StartupOptions {
  readonly withShell: boolean;
  readonly dev: boolean;
}

/** Capabilities come only from process arguments, never operator/model configuration. */
export function startupOptions(
  args: string[],
  launcher = false,
): StartupOptions {
  let withShell = false;
  let dev = false;
  for (const arg of args) {
    if (arg === "--with-shell" && !withShell) withShell = true;
    else if (arg === "--dev" && launcher && !dev) dev = true;
    else throw new Error(`Unknown or duplicate argument: ${arg}`);
  }
  return Object.freeze({ withShell, dev });
}

export function workerArguments(
  options: StartupOptions,
  root: string,
): string[] {
  return [
    "run",
    "--unstable-kv",
    "--no-prompt",
    `--config=${resolve(root, "deno.json")}`,
    options.withShell ? "-P=with-shell" : "-P=default",
    ...(options.dev ? ["--watch=src", "--no-clear-screen"] : []),
    resolve(root, "src/main.ts"),
    ...(options.withShell ? ["--with-shell"] : []),
  ];
}

export async function requireShellPermission(): Promise<void> {
  if (process.platform === "win32")
    throw new Error("--with-shell requires a POSIX shell runtime");
  const runtime = (
    globalThis as unknown as {
      Deno?: {
        permissions: {
          query(input: { name: "run" }): Promise<{ state: string }>;
        };
      };
    }
  ).Deno;
  if (
    !runtime ||
    (await runtime.permissions.query({ name: "run" })).state !== "granted"
  )
    throw new Error(
      "--with-shell requires Deno --allow-run at launch; use deno task start --with-shell",
    );
}
