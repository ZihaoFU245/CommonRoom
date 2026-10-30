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

for (const { withS3, custom, offline, network } of [
  { withS3: false, custom: false, offline: false },
  { withS3: true, custom: false, offline: false },
  { withS3: true, custom: true, offline: false },
  { withS3: true, custom: false, offline: true },
  { withS3: false, network: "100.101.102.103" },
  { withS3: false, network: "devbox.example.ts.net" },
]) {
  test(
    `debug leaves S3 startup to the caller (S3=${withS3}, overrides=${custom}, offline=${offline}, network=${network})`,
    { skip: process.platform === "win32" },
    (t) => {
      const { root } = fixture(t);
      copyFileSync(
        new URL("../../auto/debug.sh", import.meta.url),
        join(root, "auto/debug.sh"),
      );
      mkdirSync(join(root, "target/debug"), { recursive: true });
      mkdirSync(join(root, "web/node_modules/vite/bin"), { recursive: true });
      writeFileSync(join(root, "web/node_modules/vite/bin/vite.js"), "fixture");
      writeFileSync(
        join(root, "auto/start-s3.sh"),
        "#!/bin/sh\ntouch s3-was-started\n",
        { mode: 0o755 },
      );
      writeFileSync(
        join(root, "target/debug/chat"),
        `#!${process.execPath}
const fs=require('node:fs');
if(process.argv.includes('--setup-local-s3')) {fs.writeFileSync('local-s3-setup.json',JSON.stringify({endpoint:process.env.CHAT_S3_ENDPOINT,bucket:process.env.CHAT_S3_BUCKET}));process.exit(${offline ? 1 : 0});}
fs.writeFileSync('debug-env.json',JSON.stringify({s3:Object.fromEntries(Object.entries(process.env).filter(([name])=>name.startsWith('CHAT_S3_'))),stdin:fs.readFileSync(0,'utf8')}));
`,
        { mode: 0o755 },
      );
      writeFileSync(
        join(root, "bin/node"),
        `#!${process.execPath}
const fs=require('node:fs');
fs.writeFileSync('preview-env.json',JSON.stringify({args:process.argv.slice(2),origin:process.env.CHAT_DEBUG_ORIGIN,host:process.env.__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS}));
const timer=setInterval(()=>{if(fs.existsSync('debug-env.json')){clearInterval(timer);process.exit(0);}},10);
setTimeout(()=>process.exit(1),3000).unref();
`,
        { mode: 0o755 },
      );
      const env = {
        ...process.env,
        PATH: `${join(root, "bin")}:${process.env.PATH}`,
      };
      for (const name of Object.keys(env))
        if (name.startsWith("CHAT_S3_") || name.startsWith("RUSTFS_"))
          delete env[name];
      if (custom)
        Object.assign(env, {
          CHAT_S3_ENDPOINT: "http://127.0.0.1:29000",
          CHAT_S3_PUBLIC_ENDPOINT: "http://localhost:29000",
          CHAT_S3_BUCKET: "fixture-media",
          CHAT_S3_REGION: "fixture-region",
          CHAT_S3_PATH_STYLE: "false",
          RUSTFS_ACCESS_KEY: "fixture-access",
          RUSTFS_SECRET_KEY: "fixture-secret",
        });
      const result = spawnSync(
        "sh",
        [
          join(root, "auto/debug.sh"),
          ...(withS3 ? ["--with-s3"] : []),
          ...(network ? ["--network", network] : []),
        ],
        {
          cwd: tmpdir(),
          env,
          input: "/whoami\n",
          encoding: "utf8",
          timeout: 5000,
        },
      );
      assert.ifError(result.error);
      assert.equal(
        result.status,
        offline ? 1 : 0,
        result.stdout + result.stderr,
      );
      assert.equal(existsSync(join(root, "s3-was-started")), false);
      if (offline) {
        assert.equal(existsSync(join(root, "debug-env.json")), false);
        return;
      }
      const config = JSON.parse(
        readFileSync(join(root, "data/config.json"), "utf8"),
      );
      const preview = JSON.parse(
        readFileSync(join(root, "preview-env.json"), "utf8"),
      );
      const origin = `http://${network ?? "127.0.0.1"}:5173`;
      assert.equal(preview.origin, origin);
      assert.equal(preview.host, network ?? "");
      assert.equal(
        preview.args[preview.args.indexOf("--host") + 1],
        network ? "0.0.0.0" : "127.0.0.1",
      );
      assert.ok(config.origins.includes(origin));
      assert.ok(config.origins.includes("http://localhost:5173"));
      assert.match(result.stdout, new RegExp(origin.replaceAll(".", "\\.")));
      const captured = JSON.parse(
        readFileSync(join(root, "debug-env.json"), "utf8"),
      );
      assert.deepEqual(captured, {
        stdin: "/whoami\n",
        s3: !withS3
          ? {}
          : custom
            ? {
                CHAT_S3_ENDPOINT: "http://127.0.0.1:19000",
                CHAT_S3_PUBLIC_ENDPOINT: "http://127.0.0.1:19000",
                CHAT_S3_BUCKET: "fixture-media",
                CHAT_S3_REGION: "us-east-1",
                CHAT_S3_PATH_STYLE: "true",
                CHAT_S3_ACCESS_KEY: "fixture-access",
                CHAT_S3_SECRET_KEY: "fixture-secret",
              }
            : {
                CHAT_S3_ENDPOINT: "http://127.0.0.1:19000",
                CHAT_S3_PUBLIC_ENDPOINT: "http://127.0.0.1:19000",
                CHAT_S3_BUCKET: "commonroom-draft",
                CHAT_S3_REGION: "us-east-1",
                CHAT_S3_PATH_STYLE: "true",
                CHAT_S3_ACCESS_KEY: "cr",
                CHAT_S3_SECRET_KEY: "pass",
              },
      });
      if (!withS3) {
        assert.equal(existsSync(join(root, "local-s3-setup.json")), false);
        return;
      }
      assert.deepEqual(
        JSON.parse(readFileSync(join(root, "local-s3-setup.json"), "utf8")),
        {
          endpoint: captured.s3.CHAT_S3_ENDPOINT,
          bucket: captured.s3.CHAT_S3_BUCKET,
        },
      );
      assert.equal(existsSync(join(root, "s3-was-started")), false);
    },
  );
}

for (const args of [
  ["--network"],
  ["--network", "http://devbox"],
  ["--network", "devbox:5173"],
  ["--network", "0.0.0.0"],
  ["--network", 'bad"host'],
  ["--network", "--with-s3"],
]) {
  test(`debug rejects invalid network arguments: ${JSON.stringify(args)}`, (t) => {
    const { root } = fixture(t);
    copyFileSync(
      new URL("../../auto/debug.sh", import.meta.url),
      join(root, "auto/debug.sh"),
    );
    const result = spawnSync("sh", [join(root, "auto/debug.sh"), ...args], {
      encoding: "utf8",
    });
    assert.equal(result.status, 2, result.stderr);
    assert.equal(existsSync(join(root, "data")), false);
  });
}
