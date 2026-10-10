import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdtempSync,
  mkdirSync,
  copyFileSync,
  writeFileSync,
  readFileSync,
  rmSync,
  existsSync,
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
    build(mode) {
      const result = spawnSync("sh", ["./auto/build.sh", mode], {
        cwd: root,
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${join(root, "bin")}:${process.env.PATH}`,
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

for (const mode of ["debug", "release"]) {
  test(
    `${mode} build skips checks and packages the expected artifacts`,
    { skip: process.platform === "win32" },
    (t) => {
      const { root, build } = fixture(t);
      const { status, stdout, stderr, calls } = build(mode);
      assert.equal(status, 0, stdout + stderr);
      assert.ok(!calls.some((command) => checks.includes(command)));
      assert.deepEqual(calls, [
        "pnpm --dir web install --frozen-lockfile",
        mode === "debug"
          ? "pnpm --dir web exec vite build --mode development --outDir dist-debug"
          : "pnpm --dir web exec vite build",
        mode === "debug"
          ? "cargo build --locked"
          : "cargo build --release --locked",
      ]);
      assert.equal(existsSync(join(root, "ui.tar.xz")), mode === "release");
      if (mode === "release") {
        assert.equal(
          readFileSync(join(root, "chat"), "utf8"),
          "new compiled artifact",
        );
        const archive = spawnSync("tar", ["-tJf", "ui.tar.xz"], {
          cwd: root,
          encoding: "utf8",
        });
        assert.equal(archive.status, 0, archive.stderr);
        assert.deepEqual(archive.stdout.trim().split("\n").sort(), [
          "dist/",
          "dist/index.html",
        ]);
        const extracted = join(root, "extracted");
        mkdirSync(extracted);
        const extraction = spawnSync("tar", [
          "-xJf",
          join(root, "ui.tar.xz"),
          "-C",
          extracted,
        ]);
        assert.equal(extraction.status, 0);
        assert.equal(
          readFileSync(join(extracted, "dist", "index.html"), "utf8"),
          "existing web artifact",
        );
      }
    },
  );
}
