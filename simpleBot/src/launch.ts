// Select Deno permissions before the worker starts. Security/isolation belongs
// to the deployment; the application flag only enables binary execution tools.
import { resolve } from "node:path";
import { startupOptions, workerArguments } from "./startup.ts";

interface Runtime {
  Command: new (
    executable: string,
    options: {
      args: string[];
      cwd: string;
      stdin: "inherit";
      stdout: "inherit";
      stderr: "inherit";
    },
  ) => {
    spawn(): {
      status: Promise<{ code: number }>;
      kill(signal: "SIGINT" | "SIGTERM"): void;
    };
  };
}

async function launch(): Promise<void> {
  const options = startupOptions(process.argv.slice(2), true);
  const root = resolve(import.meta.dirname, "..");
  const runtime = (globalThis as unknown as { Deno?: Runtime }).Deno;
  if (!runtime) throw new Error("The worker launcher requires Deno");
  const child = new runtime.Command("deno", {
    args: workerArguments(options, root),
    cwd: root,
    stdin: "inherit",
    stdout: "inherit",
    stderr: "inherit",
  }).spawn();
  const forward = (signal: "SIGINT" | "SIGTERM") => {
    try {
      child.kill(signal);
    } catch {
      /* Already exited. */
    }
  };
  const interrupt = () => forward("SIGINT");
  const terminate = () => forward("SIGTERM");
  process.on("SIGINT", interrupt);
  process.on("SIGTERM", terminate);
  try {
    process.exitCode = (await child.status).code;
  } finally {
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", terminate);
  }
}

launch().catch((error: unknown) => {
  console.error(
    `startup failed: ${error instanceof Error ? error.message : "unknown error"}`,
  );
  process.exitCode = 1;
});
