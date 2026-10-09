import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  copyFileSync,
  writeFileSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const checks = [
  "cargo fmt --all --check",
  "cargo clippy --all-targets --locked -- -D warnings",
  "cargo test --locked",
  "pnpm --dir web check",
  "pnpm --dir web test",
];

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "commonroom-build-check-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "bin"));
  mkdirSync(join(root, "auto"));
  for (const mode of ["dist", "dist-debug"]) {
    mkdirSync(join(root, "web", mode), { recursive: true });
    writeFileSync(
      join(root, "web", mode, "index.html"),
      "existing web artifact",
    );
  }
  for (const script of ["build.sh", "check.sh"])
    copyFileSync(
      new URL(`../../auto/${script}`, import.meta.url),
      join(root, "auto", script),
    );
  writeFileSync(join(root, "Cargo.lock"), "fixture lock");
  writeFileSync(join(root, "web", "pnpm-lock.yaml"), "fixture lock");
  writeFileSync(join(root, "chat"), "existing release artifact");
  const stub = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const tool = path.basename(process.argv[1]);
const args = process.argv.slice(2);
const command = tool + ' ' + args.join(' ');
fs.appendFileSync('calls.log', command + '\\n');
if (command === process.env.FAIL_CHECK) process.exit(42);
if (tool === 'cargo' && args[0] === 'build') {
  const output = path.join('target', args.includes('--release') ? 'release' : 'debug');
  fs.mkdirSync(output, { recursive: true });
  fs.writeFileSync(path.join(output, 'chat'), 'new compiled artifact');
}
`;
  for (const tool of ["cargo", "pnpm"])
    writeFileSync(join(root, "bin", tool), stub, { mode: 0o755 });
  return {
    root,
    build(mode, failure = "") {
      const result = spawnSync("sh", ["./auto/build.sh", mode], {
        cwd: root,
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${join(root, "bin")}:${process.env.PATH}`,
          FAIL_CHECK: failure,
        },
      });
      assert.ifError(result.error);
      return {
        ...result,
        calls: readFileSync(join(root, "calls.log"), "utf8").trim().split("\n"),
      };
    },
  };
}

for (const failure of checks) {
  test(
    `build stops at failed ${failure} and preserves existing artifacts`,
    { skip: process.platform === "win32" },
    (t) => {
      const { root, build } = fixture(t);
      const { status, calls } = build("release", failure);
      assert.equal(status, 42);
      assert.equal(calls.at(-1), failure);
      assert.ok(
        !calls.some(
          (command) =>
            command.startsWith("cargo build") ||
            command.includes("exec vite build"),
        ),
      );
      assert.equal(
        readFileSync(join(root, "chat"), "utf8"),
        "existing release artifact",
      );
      for (const mode of ["dist", "dist-debug"])
        assert.equal(
          readFileSync(join(root, "web", mode, "index.html"), "utf8"),
          "existing web artifact",
        );
    },
  );
}
for (const mode of ["debug", "release"]) {
  test(
    `${mode} build ${mode === "release" ? "checks all sources first" : "skips checks"}`,
    { skip: process.platform === "win32" },
    (t) => {
      const { root, build } = fixture(t);
      const { status, stdout, stderr, calls } = build(mode);
      assert.equal(status, 0, stdout + stderr);
      const stages = mode === "release" ? checks : [];
      assert.deepEqual(calls.slice(1, 1 + stages.length), stages);
      if (mode === "debug")
        assert.ok(!calls.some((command) => checks.includes(command)));
      assert.match(calls[1 + stages.length], /^pnpm --dir web exec vite build/);
      assert.match(calls[2 + stages.length], /^cargo build/);
      if (mode === "release")
        assert.equal(
          readFileSync(join(root, "chat"), "utf8"),
          "new compiled artifact",
        );
    },
  );
}
