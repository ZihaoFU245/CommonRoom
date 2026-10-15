import assert from "node:assert/strict";
import { test } from "node:test";
import {
  mkdtempSync,
  readFileSync,
  writeFileSync,
  rmSync,
  readdirSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { ConfigStore, CONFIG_KEYS, quotedValue } from "../src/config.ts";

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "simplebot-config-"));
  const path = join(dir, "config.json");
  return {
    dir,
    path,
    cleanup: () => rmSync(dir, { recursive: true, force: true }),
  };
}

test("token thresholds are configurable, ordered and validated atomically", () => {
  const f = fixture();
  try {
    const config = ConfigStore.load(f.path);
    assert.equal(config.get("model.contextTokens"), 32768);
    assert.equal(config.get("model.compactTokens"), 24576);
    const original = readFileSync(f.path, "utf8");
    assert.throws(
      () => config.set("model.contextTokens", 24576),
      /must be less/,
    );
    assert.throws(
      () => config.set("model.compactTokens", 32768),
      /must be less/,
    );
    assert.throws(() => config.set("model.compactTokens", 1.5), /integer/);
    assert.equal(readFileSync(f.path, "utf8"), original);
    config.set("model.compactTokens", 2048);
    config.set("model.contextTokens", 4096);
    assert.equal(ConfigStore.load(f.path).get("model.contextTokens"), 4096);
    assert.equal(ConfigStore.load(f.path).get("model.compactTokens"), 2048);
    writeFileSync(
      f.path,
      JSON.stringify({ ...config.all(), "model.compactTokens": 4096 }),
    );
    assert.throws(() => config.reload(), /must be less/);
    assert.equal(config.get("model.compactTokens"), 2048);
  } finally {
    f.cleanup();
  }
});

test("config JSON and CLI round-trip quotes, slashes, Unicode, newlines and empty values", () => {
  const f = fixture();
  try {
    const prompt = '  Use "Markdown" 🙂\\paths\nwith tabs\tand spaces  ';
    const cli = join(import.meta.dirname, "../src/config-cli.ts");
    const result = spawnSync(
      process.execPath,
      ["run", "-A", cli, "prompt", prompt],
      {
        env: { ...process.env, BOT_CONFIG: f.path },
        encoding: "utf8",
      },
    );
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(result.stdout).prompt, prompt);
    assert.equal(JSON.parse(readFileSync(f.path, "utf8")).prompt, prompt);
    assert.equal(ConfigStore.load(f.path).get("prompt"), prompt);
    assert.equal(quotedValue(JSON.stringify(prompt)), prompt);
    assert.equal(quotedValue('""'), "");
    for (const raw of ["not quoted", "true", '"two" "values"', '"unterminated'])
      assert.throws(() => quotedValue(raw), /double-quoted/);
    const show = spawnSync(process.execPath, ["run", "-A", cli], {
      env: { ...process.env, BOT_CONFIG: f.path },
      encoding: "utf8",
    });
    assert.deepEqual(Object.keys(JSON.parse(show.stdout)), [...CONFIG_KEYS]);
    assert.equal(statSync(f.path).mode & 0o777, 0o600);
    assert.deepEqual(readdirSync(f.dir), ["config.json"]);
  } finally {
    f.cleanup();
  }
});

test("configuration writers merge current disk settings and reject busy writes without data loss", () => {
  const f = fixture();
  try {
    const first = ConfigStore.load(f.path),
      second = ConfigStore.load(f.path);
    first.set("prompt", "first writer");
    second.set("model.name", "second writer");
    assert.equal(ConfigStore.load(f.path).get("prompt"), "first writer");
    assert.equal(first.get("model.name"), "");
    first.reload();
    assert.equal(first.get("model.name"), "second writer");
    const before = readFileSync(f.path, "utf8");
    writeFileSync(`${f.path}.lock`, "busy");
    assert.throws(() => first.set("prompt", "lost"), /busy/);
    assert.equal(readFileSync(f.path, "utf8"), before);
    assert.equal(first.get("prompt"), "first writer");
  } finally {
    f.cleanup();
  }
});

test("invalid existing configuration is never overwritten by CLI, set or reload", () => {
  const f = fixture();
  try {
    const config = ConfigStore.load(f.path);
    config.set("prompt", "keep live settings");
    for (const invalid of [
      "{ broken",
      "null",
      "[]",
      '{"prompt":null}',
      '{"model.timeoutMs":0}',
      '{"unknown":true}',
    ]) {
      writeFileSync(f.path, invalid);
      assert.throws(() => config.reload());
      assert.throws(() => config.set("model.name", "never saved"));
      assert.throws(() => ConfigStore.load(f.path));
      const cli = spawnSync(
        process.execPath,
        [
          join(import.meta.dirname, "../src/config-cli.ts"),
          "prompt",
          "never saved",
        ],
        { env: { ...process.env, BOT_CONFIG: f.path }, encoding: "utf8" },
      );
      assert.equal(cli.status, 1);
      assert.equal(cli.stdout, "");
      assert.equal(readFileSync(f.path, "utf8"), invalid);
      assert.equal(config.get("prompt"), "keep live settings");
    }
  } finally {
    f.cleanup();
  }
});

test("reload validates atomically, notifies once and explicit runtime set replaces environment seeds", () => {
  const f = fixture();
  try {
    const config = ConfigStore.load(f.path);
    config.override("chat.token", "seed secret");
    let changes = 0;
    config.onReload(() => changes++);
    writeFileSync(
      f.path,
      JSON.stringify({ prompt: "loaded prompt", "model.stream": false }),
    );
    config.reload();
    assert.equal(changes, 1);
    assert.equal(config.get("prompt"), "loaded prompt");
    assert.equal(config.get("model.stream"), false);
    assert.equal(config.get("chat.token"), "seed secret");
    config.reload();
    assert.equal(changes, 1);
    config.set("chat.token", "new runtime secret");
    assert.equal(config.get("chat.token"), "new runtime secret");
    assert.equal(changes, 2);
    assert.ok(!JSON.stringify(config.fetch(null)).includes("runtime secret"));
  } finally {
    f.cleanup();
  }
});

test("autonomous opt-in and the configurable tool ceiling persist while old configuration defaults remain safe", () => {
  const f = fixture();
  try {
    writeFileSync(f.path, '{"model.name":"existing"}');
    const config = ConfigStore.load(f.path);
    assert.equal(config.get("autonomous"), false);
    assert.equal(config.get("model.maxToolCalls"), 32);
    config.apply("autonomous", "true");
    assert.equal(ConfigStore.load(f.path).get("autonomous"), true);
    assert.throws(() => config.apply("autonomous", "maybe"), /true or false/);
    assert.throws(() => config.apply("model.maxToolCalls", "129"), /1-128/);
  } finally {
    f.cleanup();
  }
});
