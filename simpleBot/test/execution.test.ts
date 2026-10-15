import assert from "node:assert/strict";
import { test } from "node:test";
import {
  mkdtemp,
  mkdir,
  readFile,
  writeFile,
  readdir,
  rm,
  symlink,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { createServer } from "node:http";
import { spawnSync } from "node:child_process";
import { Execution } from "../src/execution.ts";

interface Result {
  ok?: boolean;
  exitCode?: number;
  stdout?: string;
  stderr?: string;
  error?: string;
  stopped?: string;
}
async function fixture() {
  const data = await mkdtemp(join(tmpdir(), "simplebot-exec-"));
  const workspace = join(data, "workspace");
  const runner = new Execution(workspace);
  return {
    data,
    workspace,
    runner,
    run: async (
      input: unknown,
      signal = new AbortController().signal,
    ): Promise<Result> =>
      JSON.parse(await runner.execute(JSON.stringify(input), signal)),
    close: () => rm(data, { recursive: true, force: true }),
  };
}

test("execution supports TypeScript, async code, persistent files and saved module reruns", async () => {
  const f = await fixture();
  try {
    const first = await f.run({
      code: "const n: number = await Promise.resolve(21); console.log(n * 2);",
    });
    assert.equal(first.ok, true, first.error ?? first.stderr ?? "");
    assert.equal(first.stdout, "42\n");
    const saved = await f.run({
      code: `
      await Deno.mkdir("scripts", {recursive:true});
      await Deno.writeTextFile("scripts/helper.js", "export const step = 1;");
      await Deno.writeTextFile("scripts/main.ts", ${JSON.stringify('import {step} from "./helper.js"; let n = 0; try { n = Number(await Deno.readTextFile("count.txt")); } catch {} await Deno.writeTextFile("count.txt", String(n + step)); console.log(n + step);')});
      console.log("saved");`,
    });
    assert.equal(saved.ok, true, saved.error ?? saved.stderr ?? "");
    for (const count of [1, 2]) {
      const result = await f.run({ file: "scripts/main.ts" });
      assert.equal(result.ok, true, result.error ?? result.stderr ?? "");
      assert.equal(result.stdout, `${count}\n`);
    }
    assert.equal(await readFile(join(f.workspace, "count.txt"), "utf8"), "2");
    assert.deepEqual(
      await readdir(f.data),
      ["workspace"],
      "private runtime caches are removed",
    );
  } finally {
    await f.close();
  }
});

test("execution denies outside files, environment, binaries, FFI and system information", async () => {
  const f = await fixture();
  try {
    const privateFile = join(f.data, "config.json");
    await writeFile(privateFile, '{"secret":"private-fixture"}');
    const result = await f.run({
      code: `
      const denied = async (fn) => { try { await fn(); return false; } catch { return true; } };
      console.log(JSON.stringify(await Promise.all([
        denied(() => Deno.readTextFile("../config.json")),
        denied(() => Deno.writeTextFile("../outside.txt", "changed")),
        denied(() => Deno.env.get("BOT_TOKEN")),
        denied(() => new Deno.Command(${JSON.stringify(process.execPath)}, {args:["eval", "console.log(42)"]}).output()),
        denied(async () => (await import("node:fs/promises")).readFile(${JSON.stringify(privateFile)}, "utf8")),
        denied(async () => {const {createRequire} = await import("node:module"); createRequire(${JSON.stringify(pathToFileURL(join(f.workspace, "entry.ts")).href)})(${JSON.stringify(privateFile)});}),
        denied(() => Deno.permissions.request({name:"read",path:${JSON.stringify(privateFile)}}).then(p => { if(p.state === "denied") throw Error("denied"); })),
        denied(() => Deno.systemMemoryInfo()),
        denied(() => Deno.symlink(${JSON.stringify(privateFile)}, "link")),
        denied(() => Deno.link(${JSON.stringify(privateFile)}, "hardlink")),
      ])));
      console.log((await Deno.permissions.query({name:"ffi"})).state);`,
    });
    assert.equal(result.ok, true, result.error ?? result.stderr ?? "");
    assert.deepEqual(
      JSON.parse(result.stdout!.split("\n")[0]!),
      Array(10).fill(true),
    );
    assert.match(result.stdout!, /denied\n$/);
    assert.equal(
      await readFile(privateFile, "utf8"),
      '{"secret":"private-fixture"}',
    );
  } finally {
    await f.close();
  }
});

test("generated static and dynamic imports cannot load files outside workspace", async () => {
  const f = await fixture();
  try {
    const path = join(f.data, "private.ts");
    await writeFile(path, 'export default "private-fixture";');
    const url = JSON.stringify(pathToFileURL(path).href);
    for (const code of [
      `import secret from ${url}; console.log(secret);`,
      `console.log(await import(${url}));`,
    ]) {
      const result = await f.run({ code });
      assert.equal(result.ok, false);
      assert.match(result.stderr ?? "", /Requires read access/);
      assert.ok(!result.stdout?.includes("private-fixture"));
    }
    await writeFile(
      join(f.workspace, "outside-import.ts"),
      `import secret from ${url}; console.log(secret);`,
    );
    const saved = await f.run({ file: "outside-import.ts" });
    assert.equal(saved.ok, false);
    assert.match(saved.stderr ?? "", /Requires read access/);
    const rejected = await f.run({ file: "../private.ts" });
    assert.match(rejected.error ?? "", /inside the execution workspace/);
  } finally {
    await f.close();
  }
});

test("workspace paths with spaces and commas remain correctly scoped", async () => {
  const f = await fixture();
  try {
    const workspace = join(f.data, "workspace, shared folder");
    const runner = new Execution(workspace);
    const result: Result = JSON.parse(
      await runner.execute(
        JSON.stringify({
          code: 'await Deno.writeTextFile("answer.txt", "42"); console.log(await Deno.readTextFile("answer.txt"));',
        }),
        new AbortController().signal,
      ),
    );
    assert.equal(result.ok, true, result.error ?? result.stderr ?? "");
    assert.equal(await readFile(join(workspace, "answer.txt"), "utf8"), "42");
  } finally {
    await f.close();
  }
});

test("execution refuses pre-existing symlinks that escape the workspace", async () => {
  const f = await fixture();
  try {
    await mkdir(f.workspace);
    await writeFile(join(f.data, "private.json"), "private-fixture");
    await symlink(join(f.data, "private.json"), join(f.workspace, "escape"));
    const result = await f.run({
      code: 'console.log(await Deno.readTextFile("escape"));',
    });
    assert.match(result.error ?? "", /symbolic links/);
  } finally {
    await f.close();
  }
});

test("execution bounds stdout/stderr, kills infinite loops and handles cancellation", async () => {
  const f = await fixture();
  try {
    const timeout = await f.run({ code: "while(true) {}", timeoutMs: 100 });
    assert.equal(timeout.stopped, "timeout");
    assert.equal(timeout.ok, false);
    const output = await f.run({
      code: 'while(true) { console.log("x".repeat(4096)); console.error("e".repeat(4096)); }',
    });
    assert.equal(output.stopped, "output_limit");
    assert.ok(
      Buffer.byteLength((output.stdout ?? "") + (output.stderr ?? "")) <= 32768,
    );
    const controller = new AbortController();
    const execution = f.run(
      { code: 'await Deno.writeTextFile("started", "yes"); while(true) {}' },
      controller.signal,
    );
    for (let i = 0; i < 100; i++) {
      try {
        await readFile(join(f.workspace, "started"));
        break;
      } catch {
        await new Promise((done) => setTimeout(done, 10));
      }
    }
    controller.abort();
    assert.equal((await execution).stopped, "cancelled");
    assert.equal(
      (await f.run({ code: 'console.log("ready");' })).stdout,
      "ready\n",
    );
    const alreadyCancelled = await f.run(
      { code: 'await Deno.writeTextFile("late", "bad");' },
      controller.signal,
    );
    assert.match(alreadyCancelled.error ?? "", /cancelled/);
    assert.ok(!(await readdir(f.workspace)).includes("late"));
  } finally {
    await f.close();
  }
});

test("execution rejects invalid requests before running anything", async () => {
  const f = await fixture();
  try {
    for (const input of [
      {},
      { code: "" },
      { code: "1", file: "a.ts" },
      { file: "/etc/script.ts" },
      { code: "1", timeoutMs: 0 },
      { code: "1", timeoutMs: 30001 },
      { code: "1", args: ["-A"] },
      { code: "x".repeat(48001) },
    ])
      assert.ok((await f.run(input)).error);
    assert.deepEqual(await readdir(f.data), []);
  } finally {
    await f.close();
  }
});

test("execution can fetch HTTP and import remote modules without inherited credentials", async () => {
  const f = await fixture();
  const server = createServer((request, response) => {
    assert.equal(request.headers.authorization, undefined);
    response.setHeader(
      "Content-Type",
      request.url === "/module.ts"
        ? "application/typescript"
        : "application/json",
    );
    response.end(
      request.url === "/module.ts"
        ? "export const answer: number = 42;"
        : '{"answer":42}',
    );
  });
  await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
  try {
    const address = server.address();
    assert.ok(address && typeof address !== "string");
    const url = `http://127.0.0.1:${address.port}`;
    const result = await f.run({
      code: `const response = await fetch(${JSON.stringify(url)}); console.log((await response.json()).answer); const module = await import(${JSON.stringify(url + "/module.ts")}); console.log(module.answer);`,
    });
    assert.equal(result.ok, true, result.error ?? result.stderr ?? "");
    assert.equal(result.stdout, "42\n42\n");
  } finally {
    await new Promise<void>((done) => server.close(() => done()));
    await f.close();
  }
});

test("the worker permission profile can launch the restricted child", async () => {
  const f = await fixture();
  try {
    const source = pathToFileURL(
      join(import.meta.dirname, "../src/execution.ts"),
    ).href;
    const parent = join(f.data, "parent.ts");
    const config = join(f.data, "deno.json");
    const template = JSON.parse(
      await readFile(join(import.meta.dirname, "../deno.json"), "utf8"),
    );
    template.permissions.default.read = [f.data];
    template.permissions.default.write = [f.data];
    await writeFile(config, JSON.stringify(template));
    await writeFile(
      parent,
      `import {Execution} from ${JSON.stringify(source)}; console.log(await new Execution(${JSON.stringify(f.workspace)}).execute('{"code":"console.log(42)"}', new AbortController().signal));`,
    );
    const result = spawnSync(
      process.execPath,
      ["run", "--no-prompt", "--config", config, "-P", parent],
      { encoding: "utf8", timeout: 15000 },
    );
    assert.equal(result.status, 0, result.stderr);
    const execution: Result = JSON.parse(result.stdout);
    assert.equal(execution.ok, true, execution.error ?? execution.stderr ?? "");
    assert.equal(execution.stdout, "42\n");
  } finally {
    await f.close();
  }
});

test(
  "execution can fetch public HTTPS",
  { skip: process.env.BOT_EXEC_HTTPS !== "1" },
  async () => {
    const f = await fixture();
    try {
      const result = await f.run({
        code: 'const response = await fetch("https://docs.deno.com/"); console.log(response.status); await response.body?.cancel();',
      });
      assert.equal(result.ok, true, result.error ?? result.stderr ?? "");
      assert.equal(result.stdout, "200\n");
    } finally {
      await f.close();
    }
  },
);
