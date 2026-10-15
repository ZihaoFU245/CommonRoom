import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { Memory } from "../src/memory.ts";
import { Store } from "../src/store.ts";
import { Records } from "../src/records.ts";

test("model workspace survives restart, supports arbitrary JSON and isolates rooms, DMs and accounts", async () => {
  const dir = await mkdtemp(join(tmpdir(), "simplebot-memory-"));
  const path = join(dir, "agent.kv");
  let store = await Store.open(path);
  try {
    // An obsolete JSON checkpoint cannot seed or corrupt the new database.
    await writeFile(join(dir, "checkpoint.json"), "broken old state");
    const memory = new Memory(store.records.kv, "bot", "room", [
      { id: "m1", authorId: "alice", hash: "original" },
    ]);
    const saved = JSON.parse(
      await memory.execute(
        JSON.stringify({
          action: "set",
          key: "index",
          value: { topics: ["runtime"] },
          version: null,
        }),
      ),
    );
    assert.equal(saved.ok, true);
    assert.equal(typeof saved.version, "string");
    await memory.execute(
      JSON.stringify({
        action: "set",
        key: "topics/runtime",
        value: ["Deno", { agreed: false }],
      }),
    );
    await store.close();
    store = await Store.open(path);
    const restored = new Memory(store.records.kv, "bot", "room");
    const entry = JSON.parse(
      await restored.execute('{"action":"get","key":"index"}'),
    );
    assert.deepEqual(entry.value, { topics: ["runtime"] });
    assert.equal(entry.sources[0].authorId, "alice");
    assert.match(await restored.context(), /topics\/runtime/);
    for (const scoped of [
      new Memory(store.records.kv, "bot", "dm"),
      new Memory(store.records.kv, "other-bot", "room"),
    ]) {
      assert.equal(
        JSON.parse(await scoped.execute('{"action":"get","key":"index"}'))
          .version,
        null,
      );
      assert.ok(!String(await scoped.context()).includes("runtime"));
    }
    const conflict = JSON.parse(
      await restored.execute(
        '{"action":"set","key":"index","value":"stale","version":null}',
      ),
    );
    assert.match(conflict.error, /version_conflict/);
    assert.equal(
      JSON.parse(
        await restored.execute(
          JSON.stringify({
            action: "delete",
            key: "index",
            version: entry.version,
          }),
        ),
      ).ok,
      true,
    );
    assert.equal(
      JSON.parse(await restored.execute('{"action":"get","key":"index"}'))
        .version,
      null,
    );
    assert.equal((await stat(path)).mode & 0o777, 0o600);
  } finally {
    await store.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test("workspace pagination, conditional writes and validation preserve existing documents", async () => {
  const store = await Store.open(":memory:");
  try {
    const memory = new Memory(store.records.kv, "bot", "room");
    for (let i = 0; i < 5; i++)
      assert.equal(
        JSON.parse(
          await memory.execute(
            JSON.stringify({ action: "set", key: `topic/${i}`, value: i }),
          ),
        ).ok,
        true,
      );
    const first = JSON.parse(
      await memory.execute('{"action":"list","prefix":"topic/","limit":2}'),
    );
    const next = JSON.parse(
      await memory.execute(
        JSON.stringify({
          action: "list",
          prefix: "topic/",
          limit: 2,
          cursor: first.cursor,
        }),
      ),
    );
    assert.equal(first.documents.length, 2);
    assert.equal(next.documents.length, 2);
    assert.ok(
      !first.documents.some((a: { key: string }) =>
        next.documents.some((b: { key: string }) => a.key === b.key),
      ),
    );
    const original = JSON.parse(
      await memory.execute('{"action":"get","key":"topic/0"}'),
    );
    const updated = JSON.parse(
      await memory.execute(
        JSON.stringify({
          action: "set",
          key: "topic/0",
          value: "new",
          version: original.version,
        }),
      ),
    );
    assert.equal(updated.ok, true);
    assert.match(
      JSON.parse(
        await memory.execute(
          JSON.stringify({
            action: "delete",
            key: "topic/0",
            version: original.version,
          }),
        ),
      ).error,
      /version_conflict/,
    );
    for (const args of [
      { action: "set", key: "index", value: "x".repeat(7000) },
      { action: "list", limit: 1000 },
      { action: "set", key: "x" },
      { action: "set", key: "x", value: 1, conversation: "other" },
    ])
      assert.ok(JSON.parse(await memory.execute(JSON.stringify(args))).error);
    const cancelled = new Memory(
      store.records.kv,
      "bot",
      "room",
      [],
      () => false,
    );
    assert.ok(
      JSON.parse(await cancelled.execute('{"action":"delete","key":"topic/0"}'))
        .error,
    );
    assert.equal(
      JSON.parse(await memory.execute('{"action":"get","key":"topic/0"}'))
        .value,
      "new",
    );
  } finally {
    await store.close();
  }
});

test("worker commits update only dirty records, including nested mutations", async () => {
  const store = await Store.open(":memory:");
  try {
    store.setCursor("a", 1);
    store.setCursor("b", 2);
    store.remember("a", "message", "before", 1);
    await store.save();
    const unchanged = await store.records.kv.get(["worker", "cursors", "b"]);
    store.setCursor("a", 3);
    store.memory("a").known.message = "after";
    await store.save();
    assert.equal(
      (await store.records.kv.get(["worker", "cursors", "b"])).versionstamp,
      unchanged.versionstamp,
    );
    const rows = new Map<string, unknown>();
    for await (const [field, id, value] of store.records.entries())
      rows.set(`${field}:${id}`, value);
    assert.equal(rows.get("cursors:a"), 3);
    assert.deepEqual(rows.get("memory:a"), {
      known: { message: "after" },
      sequence: 1,
      latest: 1,
    });
  } finally {
    await store.close();
  }
});

test("large response plans publish atomically and interrupted staging leaves the previous state intact", async () => {
  const records = await Records.open(":memory:");
  try {
    for await (const _ of records.entries()) {
      /* Initialize revision. */
    }
    await records.commit([
      { field: "test", id: "plan", json: JSON.stringify("old") },
    ]);
    const original = records.kv.set.bind(records.kv);
    let chunks = 0;
    records.kv.set = async (key, value) => {
      if (key[0] === "blob" && ++chunks === 2)
        throw new Error("injected staging failure");
      return original(key, value);
    };
    await assert.rejects(
      () =>
        records.commit([
          {
            field: "test",
            id: "plan",
            json: JSON.stringify("中".repeat(50000)),
          },
        ]),
      /injected/,
    );
    let values = [];
    for await (const row of records.entries()) values.push(row[2]);
    assert.deepEqual(values, ["old"]);
    records.kv.set = original;
    await records.commit([
      { field: "test", id: "plan", json: JSON.stringify("中".repeat(50000)) },
    ]);
    values = [];
    for await (const row of records.entries()) values.push(row[2]);
    assert.deepEqual(values, ["中".repeat(50000)]);
  } finally {
    records.close();
  }
});

test("a stale second worker cannot overwrite a committed transition", async () => {
  const dir = await mkdtemp(join(tmpdir(), "simplebot-conflict-"));
  const path = join(dir, "agent.kv");
  const first = await Records.open(path),
    second = await Records.open(path);
  try {
    for await (const _ of first.entries()) {
      /* Pin revision. */
    }
    for await (const _ of second.entries()) {
      /* Pin the same revision. */
    }
    await first.commit([{ field: "test", id: "cursor", json: "2" }]);
    await assert.rejects(
      () => second.commit([{ field: "test", id: "cursor", json: "1" }]),
      /only one worker/,
    );
  } finally {
    first.close();
    second.close();
    await rm(dir, { recursive: true, force: true });
  }
});

test("large cleanup is bounded without separating tasks from their outbox", async () => {
  const store = await Store.open(":memory:");
  store.taskLimit = 5000;
  try {
    for (let batch = 0; batch < 12; batch++) {
      for (let i = 0; i < 100; i++) {
        const id = `${batch}-${i}`;
        const conversation = `room-${i}`;
        store.setCursor(conversation, batch + 1);
        const full = `${conversation}:${id}:answer`;
        store.putTask({
          conversation,
          input: id,
          sequence: batch + 1,
          promptHash: "hash",
          replyTo: null,
          status: "answered",
          attempts: 1,
          slots: ["answer"],
          updated: batch,
        });
        store.putPart({
          slot: full,
          key: "key",
          request: { conversation_id: conversation, text: "sent" },
          created: 1,
          response: {
            message: { id: "reply", sequence: batch + 2 },
            replayed: false,
          },
        });
      }
      await store.save();
    }
    for (let round = 0; round < 20 && store.tasks().length; round++) {
      assert.ok(store.forget(new Set()).length > 0);
      await store.save();
      for (const task of store.tasks())
        for (const slot of task.slots)
          assert.ok(
            store.part(`${task.conversation}:${task.input}:${slot}`)?.response,
          );
    }
    assert.equal(store.tasks().length, 0);
    assert.equal(Object.keys(store.state.parts).length, 0);
    assert.equal(Object.keys(store.state.cursors).length, 0);
  } finally {
    await store.close();
  }
});

test("failed publication preserves deletions and updates together and poisons later worker commits", async () => {
  const store = await Store.open(":memory:");
  store.setCursor("old", 1);
  await store.save();
  const atomic = store.records.kv.atomic.bind(store.records.kv);
  store.records.kv.atomic = () => {
    const operation = atomic();
    operation.commit = async () => ({ ok: false });
    return operation;
  };
  delete store.state.cursors.old;
  store.setCursor("new", 2);
  await assert.rejects(() => store.save(), /only one worker/);
  store.records.kv.atomic = atomic;
  store.setCursor("later", 3);
  await assert.rejects(() => store.save(), /only one worker/);
  const cursors = [];
  for await (const [field, id, value] of store.records.entries())
    if (field === "cursors") cursors.push([id, value]);
  assert.deepEqual(cursors, [["old", 1]]);
  await assert.rejects(() => store.close(), /only one worker/);
});

test("a full group batch can write a concise index with all speaker provenance", async () => {
  const store = await Store.open(":memory:");
  try {
    const sources = Array.from({ length: 100 }, (_, i) => ({
      id: `message-${i}-${"x".repeat(36)}`,
      authorId: `account-${i}-${"x".repeat(36)}`,
      hash: "abc12345",
    }));
    const memory = new Memory(store.records.kv, "bot", "room", sources);
    assert.equal(
      JSON.parse(
        await memory.execute(
          '{"action":"set","key":"index","value":{"proposal":"Deno","status":"under discussion"}}',
        ),
      ).ok,
      true,
    );
    assert.equal(
      JSON.parse(await memory.execute('{"action":"get","key":"index"}')).sources
        .length,
      100,
    );
  } finally {
    await store.close();
  }
});

test("an uncertain commit retains chunks needed by the published response plan", async () => {
  const records = await Records.open(":memory:");
  try {
    for await (const _ of records.entries()) {
      /* Pin revision. */
    }
    const atomic = records.kv.atomic.bind(records.kv);
    records.kv.atomic = () => {
      const operation = atomic();
      const commit = operation.commit.bind(operation);
      operation.commit = async () => {
        await commit();
        throw new Error("lost commit acknowledgement");
      };
      return operation;
    };
    const plan = "large response plan ".repeat(10000);
    await assert.rejects(
      () =>
        records.commit([
          { field: "test", id: "plan", json: JSON.stringify(plan) },
        ]),
      /lost commit acknowledgement/,
    );
    const values = [];
    for await (const row of records.entries()) values.push(row[2]);
    assert.deepEqual(values, [plan]);
  } finally {
    records.close();
  }
});
