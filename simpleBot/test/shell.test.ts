import assert from "node:assert/strict";
import { test } from "node:test";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  rm,
  access,
  realpath,
} from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";
import { Shell, shellRequest, SHELL_TOOL } from "../src/shell.ts";
import type { ShellResult } from "../src/shell-process.ts";
import { startupOptions, workerArguments } from "../src/startup.ts";
import { modelTools } from "../src/tools.ts";
import { ConfigStore } from "../src/config.ts";

async function fixture(enabled = true) {
  const dir = await mkdtemp(join(tmpdir(), "simplebot-shell-"));
  const workspace = join(dir, "workspace");
  const shell = new Shell(workspace, enabled);
  return {
    dir,
    workspace,
    shell,
    async run(
      input: unknown,
      signal = new AbortController().signal,
    ): Promise<ShellResult> {
      return JSON.parse(await shell.execute(JSON.stringify(input), signal));
    },
    async close() {
      await shell.settle();
      await rm(dir, { recursive: true, force: true });
    },
  };
}

test("shell is enabled only by immutable launch arguments, never config", async () => {
  assert.equal(startupOptions([]).withShell, false);
  const enabled = startupOptions(["--with-shell"]);
  assert.equal(enabled.withShell, true);
  assert.ok(Object.isFrozen(enabled));
  for (const args of [
    ["--with-shell=true"],
    ["--with-shell", "--with-shell"],
    ["--dev"],
    ["--shell"],
  ])
    assert.throws(() => startupOptions(args));
  const root = join(tmpdir(), "worker-root");
  assert.ok(workerArguments(enabled, root).includes("-P=with-shell"));
  assert.ok(workerArguments(startupOptions([]), root).includes("-P=default"));
  assert.ok(
    workerArguments(
      startupOptions(["--dev", "--with-shell"], true),
      root,
    ).includes("--watch=src"),
  );
  const tools = { group: false, commands: false, history: false, reads: false };
  assert.ok(
    !modelTools(tools).some((tool) => tool.function.name === "run_shell"),
  );
  assert.ok(modelTools({ ...tools, shell: true }).includes(SHELL_TOOL));
  const f = await fixture(false);
  try {
    await assert.rejects(f.run({ command: "printf disabled" }), /--with-shell/);
    await writeFile(join(f.dir, "config.json"), '{"withShell":true}');
    assert.throws(
      () => ConfigStore.load(join(f.dir, "config.json")),
      /unknown configuration key/,
    );
  } finally {
    await f.close();
  }
});

test("shell runs binaries, pipes and redirects with workspace cwd and persistent files", async () => {
  const f = await fixture();
  try {
    const result = await f.run({
      command:
        "printf 'hello\\n' > result.txt; /bin/cat result.txt | /usr/bin/tr a-z A-Z; /bin/pwd",
    });
    assert.equal(result.ok, true, result.error ?? result.stderr);
    assert.equal(result.stdout, `HELLO\n${await realpath(f.workspace)}\n`);
    assert.equal(
      await readFile(join(f.workspace, "result.txt"), "utf8"),
      "hello\n",
    );
    const failure = await f.run({
      command: "printf out; printf err >&2; exit 7",
    });
    assert.equal(failure.ok, false);
    assert.equal(failure.exitCode, 7);
    assert.equal(failure.stdout, "out");
    assert.equal(failure.stderr, "err");
  } finally {
    await f.close();
  }
});

test("shell allows parent paths, absolute cwd and cd home according to the deployment", async () => {
  const f = await fixture();
  try {
    const other = join(f.dir, "other");
    await mkdir(other);
    await writeFile(join(other, "visible.txt"), "deployment-visible");
    for (const input of [
      { command: "/bin/cat visible.txt", cwd: "../other" },
      { command: "/bin/cat visible.txt", cwd: other },
      { command: "cd ../other && /bin/cat visible.txt" },
    ]) {
      const result = await f.run(input);
      assert.equal(result.ok, true, result.error ?? result.stderr);
      assert.equal(result.stdout, "deployment-visible");
    }
    for (const input of [
      { command: "cd ~ && /bin/pwd" },
      { command: "/bin/pwd", cwd: "~" },
    ]) {
      const result = await f.run(input);
      assert.equal(result.ok, true, result.error ?? result.stderr);
      assert.equal(result.stdout?.trim(), await realpath(homedir()));
    }
  } finally {
    await f.close();
  }
});

test("wait expiry yields while the command continues; quick commands return before expiry", async () => {
  const f = await fixture();
  try {
    let yielded = false;
    const quick: ShellResult = JSON.parse(
      await f.shell.execute(
        '{"command":"printf quick","waitMs":1000}',
        new AbortController().signal,
        async () => {
          yielded = true;
        },
      ),
    );
    assert.equal(quick.stdout, "quick");
    assert.equal(yielded, false);
    let notify: (() => void) | undefined;
    const notice = new Promise<void>((resolve) => {
      notify = resolve;
    });
    let completed = false;
    const work = f.shell
      .execute(
        '{"command":"sleep 0.2; printf done","waitMs":10}',
        new AbortController().signal,
        async () => {
          yielded = true;
          notify!();
        },
      )
      .then((result) => {
        completed = true;
        return JSON.parse(result) as ShellResult;
      });
    await notice;
    assert.equal(yielded, true);
    assert.equal(completed, false);
    const result = await work;
    assert.equal(result.ok, true);
    assert.equal(result.stdout, "done");
  } finally {
    await f.close();
  }
});

test("timeouts and cancellation stop ordinary process-group children and bound output", async () => {
  const f = await fixture();
  try {
    const timeout = await f.run({
      command: "(sleep 0.4; printf orphan > orphan.txt) & wait",
      waitMs: 0,
      timeoutMs: 100,
    });
    assert.equal(timeout.stopped, "timeout");
    const controller = new AbortController();
    const work = f.run({ command: "sleep 30", waitMs: 0 }, controller.signal);
    const timer = setTimeout(() => controller.abort(), 30);
    try {
      assert.equal((await work).stopped, "cancelled");
    } finally {
      clearTimeout(timer);
    }
    const large = await f.run({
      command: "while :; do printf '0123456789abcdef'; done",
    });
    assert.equal(large.stopped, "output_limit");
    assert.ok(
      (large.stdout?.length ?? 0) + (large.stderr?.length ?? 0) <= 32768,
    );
    await new Promise((resolve) => setTimeout(resolve, 450));
    await assert.rejects(access(join(f.workspace, "orphan.txt")));
  } finally {
    await f.close();
  }
});

test("shell rejects malformed arguments and reports invalid working directories", async () => {
  for (const input of [
    {},
    { command: " " },
    { command: "x", waitMs: -1 },
    { command: "x", timeoutMs: 1 },
    { command: "x", cwd: "\0" },
    { command: "x", extra: true },
  ])
    assert.throws(() => shellRequest(JSON.stringify(input)));
  const f = await fixture();
  try {
    const result = await f.run({
      command: "printf missing",
      cwd: "missing-directory",
    });
    assert.equal(result.ok, false);
    assert.match(result.error ?? "", /ENOENT/);
  } finally {
    await f.close();
  }
});

test("launch permission profiles grant binary execution only for --with-shell", async () => {
  const f = await fixture();
  try {
    const template = JSON.parse(
      await readFile(join(import.meta.dirname, "../deno.json"), "utf8"),
    );
    for (const profile of ["default", "with-shell"]) {
      template.permissions[profile].read = [f.dir];
      template.permissions[profile].write = [f.dir];
    }
    const config = join(f.dir, "deno.json");
    await writeFile(config, JSON.stringify(template));
    const source = pathToFileURL(
      join(import.meta.dirname, "../src/shell.ts"),
    ).href;
    const startup = pathToFileURL(
      join(import.meta.dirname, "../src/startup.ts"),
    ).href;
    const parent = join(f.dir, "parent.ts");
    await writeFile(
      parent,
      `import {Shell} from ${JSON.stringify(source)}; import {requireShellPermission} from ${JSON.stringify(startup)}; await requireShellPermission(); console.log(await new Shell(${JSON.stringify(f.workspace)}, true).execute('{"command":"printf granted"}', new AbortController().signal));`,
    );
    const denied = spawnSync(
      process.execPath,
      ["run", "--no-prompt", "--config", config, "-P=default", parent],
      { encoding: "utf8", timeout: 15000 },
    );
    assert.notEqual(denied.status, 0);
    assert.match(denied.stderr, /requires Deno --allow-run/);
    const allowed = spawnSync(
      process.execPath,
      ["run", "--no-prompt", "--config", config, "-P=with-shell", parent],
      { encoding: "utf8", timeout: 15000 },
    );
    assert.equal(allowed.status, 0, allowed.stderr);
    assert.equal((JSON.parse(allowed.stdout) as ShellResult).stdout, "granted");
  } finally {
    await f.close();
  }
});

test("the task launcher selects permissions and forwards --with-shell to the worker", async () => {
  const f = await fixture();
  try {
    await mkdir(join(f.dir, "src"));
    for (const name of ["startup.ts", "launch.ts"])
      await writeFile(
        join(f.dir, "src", name),
        await readFile(join(import.meta.dirname, "../src", name)),
      );
    const config = JSON.parse(
      await readFile(join(import.meta.dirname, "../deno.json"), "utf8"),
    );
    for (const profile of ["default", "with-shell"]) {
      config.permissions[profile].read = [f.dir];
      config.permissions[profile].write = [f.dir];
    }
    await writeFile(join(f.dir, "deno.json"), JSON.stringify(config));
    await writeFile(
      join(f.dir, "src/main.ts"),
      `import {startupOptions,requireShellPermission} from "./startup.ts"; const options=startupOptions(Deno.args); if(options.withShell) await requireShellPermission(); console.log(options.withShell ? "shell-enabled" : "shell-disabled");`,
    );
    for (const enabled of [false, true]) {
      const result = spawnSync(
        process.execPath,
        [
          "task",
          "--config",
          join(f.dir, "deno.json"),
          "start",
          ...(enabled ? ["--with-shell"] : []),
        ],
        { encoding: "utf8", timeout: 15000 },
      );
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, enabled ? /shell-enabled/ : /shell-disabled/);
    }
  } finally {
    await f.close();
  }
});
