import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AgentClient, BotApi, BotApiError } from "../scripts/bot/client.mjs";

const message = (sequence, author = "human") => ({
  id: `m-${sequence}`,
  from: author,
  author_id: author,
  text: "question",
  markdown: false,
  edited: false,
  time: 1,
  sequence,
  reactions: {},
  reply: null,
  mentions: ["helper"],
});
const snapshot = (through = 0, id = "room-id", view = "support") => ({
  kind: "snapshot",
  account: { id: "bot-id", username: "helper" },
  credential: {
    id: "credential-id",
    expires: null,
    conversations: null,
    operations: null,
  },
  conversations: [
    {
      id,
      view,
      content_revision: 1,
      permissions: { send: true, reply: true, react: true },
      members: ["helper"],
      unread: {
        count: through,
        first: through ? 1 : null,
        through,
        oldest: through ? 1 : 0,
        revision: through,
      },
    },
  ],
});
const page = (messages, hasAfter = false, id = "room-id") => ({
  conversation_id: id,
  messages,
  content_revision: 1,
  has_before: false,
  has_after: hasAfter,
});
async function fixture(fn) {
  const folder = await mkdtemp(join(tmpdir(), "bot-client-"));
  try {
    await fn(join(folder, "state.json"));
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
}

test("keyed sends and desired-state operations retry safely with retry-after", async () => {
  const bodies = [],
    waits = [];
  let calls = 0;
  const api = new BotApi({
    url: "https://chat.example.com/commonroom/",
    token: "secret",
    sleep: async (ms) => waits.push(ms),
    fetchImpl: async (url, options) => {
      assert.equal(url.pathname, "/commonroom/api/bot/v1/messages");
      assert.equal(options.headers["Idempotency-Key"], "stable");
      bodies.push(options.body);
      calls++;
      if (calls === 1) throw new TypeError("lost response");
      if (calls === 2)
        return Response.json(
          { error: { code: "rate_limited", message: "wait", retryable: true } },
          { status: 429, headers: { "Retry-After": "10" } },
        );
      return Response.json({ message: message(1, "bot-id"), replayed: true });
    },
  });
  assert.equal(
    (
      await api.request("messages", {
        body: { conversation_id: "room-id", text: "answer" },
        key: "stable",
      })
    ).replayed,
    true,
  );
  assert.equal(bodies[0], bodies[2]);
  assert.deepEqual(waits, [1000, 10000]);
  calls = 0;
  api.fetch = async () => {
    calls++;
    return Response.json(
      {
        error: {
          code: "idempotency_conflict",
          message: "conflict",
          retryable: false,
        },
      },
      { status: 409 },
    );
  };
  await assert.rejects(
    api.request("messages", {
      body: { conversation_id: "room-id", text: "answer" },
      key: "stable",
    }),
    (e) => e instanceof BotApiError && e.code === "idempotency_conflict",
  );
  assert.equal(calls, 1);
  await assert.rejects(
    api.request("messages", {
      body: { conversation_id: "room-id", text: "answer" },
    }),
    /Invalid IdempotencyKey/,
  );
});

test("catch-up paginates missing messages, deduplicates signals and keeps identity through rename", async () =>
  fixture(async (statePath) => {
    const received = [],
      queries = [];
    const messages = Array.from({ length: 205 }, (_, i) => message(i + 1));
    const api = {
      request: async (path) => {
        const query = new URL(path, "http://localhost").searchParams;
        assert.equal(query.get("conversation_id"), "room-id");
        const after = Number(query.get("after"));
        queries.push(after);
        const batch = messages.filter((m) => m.sequence > after).slice(0, 100);
        return page(batch, batch.at(-1)?.sequence < 205);
      },
    };
    const client = new AgentClient({
      api,
      statePath,
      replayHistory: true,
      onMessage: async (m) => received.push(m.sequence),
    });
    await client.sync(snapshot(205));
    assert.deepEqual(queries, [0, 100, 200]);
    assert.equal(received.length, 205);
    await client.sync(snapshot(205));
    assert.equal(received.length, 205);
    const restarted = new AgentClient({
      api,
      statePath,
      onMessage: async () => assert.fail("already processed"),
    });
    await restarted.load();
    await restarted.sync(snapshot(205, "room-id", "renamed"));
    assert.equal(restarted.state.cursors["room-id"], 205);
  }));

test("first baseline skips retained input and own messages advance without invoking the handler", async () =>
  fixture(async (statePath) => {
    const client = new AgentClient({
      statePath,
      onMessage: async () => assert.fail("own message"),
      api: {
        request: async (path) => {
          assert.equal(
            new URL(path, "http://localhost").searchParams.get("after"),
            "20",
          );
          return page([message(21, "bot-id")]);
        },
      },
    });
    await client.sync(snapshot(20));
    assert.equal(client.state.cursors["room-id"], 21);
  }));

test("crash recovery preserves the original payload and key until the input cursor commits", async () =>
  fixture(async (statePath) => {
    let lost = false;
    const sent = [];
    const api = {
      request: async (path, options) => {
        if (path.startsWith("history")) return page([message(1)]);
        const saved = JSON.parse(await readFile(statePath, "utf8"));
        assert.equal(saved.cursors["room-id"], 0);
        assert.equal(Object.values(saved.outbox)[0].key, options.key);
        sent.push({ body: structuredClone(options.body), key: options.key });
        if (!lost) {
          lost = true;
          throw new TypeError("lost acknowledgement");
        }
        return { message: message(2, "bot-id"), replayed: true };
      },
    };
    const first = new AgentClient({
      api,
      statePath,
      replayHistory: true,
      onMessage: async (m, { client }) =>
        client.send("original answer", { replyTo: m.id }),
    });
    await assert.rejects(first.sync(snapshot(1)), TypeError);
    const next = new AgentClient({
      api,
      statePath,
      onMessage: async (m, { client }) =>
        client.send("different model answer", { replyTo: m.id }),
    });
    await next.load();
    await next.sync(snapshot(1));
    assert.deepEqual(sent[0], sent[1]);
    const saved = JSON.parse(await readFile(statePath, "utf8"));
    assert.equal(saved.cursors["room-id"], 1);
    assert.deepEqual(saved.outbox, {});
  }));

test("expired ambiguous sends and corrupt or mismatched checkpoints stop without resending", async () =>
  fixture(async (statePath) => {
    const state = {
      version: 2,
      revisions: {},
      account: "bot-id",
      cursors: { "room-id": 0 },
      outbox: {
        "room-id:m-1:answer": {
          key: "stable",
          input: { conversation_id: "room-id", text: "answer", markdown: true },
          source: "room-id:m-1",
          sequence: 1,
          created: Date.now() - 24 * 60 * 60 * 1000,
        },
      },
    };
    await writeFile(statePath, JSON.stringify(state));
    const api = {
      request: async (path) => {
        assert.ok(path.startsWith("history"));
        return page([message(1)]);
      },
    };
    const client = new AgentClient({
      api,
      statePath,
      onMessage: async (_m, { client }) => client.send("answer"),
    });
    await client.load();
    await assert.rejects(client.sync(snapshot(1)), /near receipt expiry/);
    assert.equal(client.state.cursors["room-id"], 0);
    await assert.rejects(
      client.sync({
        ...snapshot(1),
        account: { id: "another-bot", username: "helper" },
      }),
      /another account/,
    );
    state.outbox["room-id:m-1:answer"].response = {
      message: { id: "broken" },
      replayed: true,
    };
    await writeFile(statePath, JSON.stringify(state));
    await assert.rejects(client.load(), /Invalid SendResponse/);
  }));

test("opaque identifiers do not inherit object properties", async () =>
  fixture(async (statePath) => {
    const client = new AgentClient({
      statePath,
      api: { request: async () => page([], false, "__proto__") },
      onMessage: async () => assert.fail("no input"),
    });
    await client.sync(snapshot(0, "__proto__"));
    assert.equal(Object.hasOwn(client.state.cursors, "__proto__"), true);
    assert.equal(client.state.cursors["__proto__"], 0);
    const restored = new AgentClient({ statePath });
    await restored.load();
    assert.equal(Object.getPrototypeOf(restored.state.cursors), null);
    assert.equal(restored.state.cursors["__proto__"], 0);
  }));

test("revision callbacks survive restart and run even with no new sequences", async () =>
  fixture(async (statePath) => {
    const revisions = [];
    const options = {
      statePath,
      api: { request: async () => page([]) },
      onMessage: async () => assert.fail("no new input"),
      onRevision: async (change) => revisions.push(change.revision),
    };
    const client = new AgentClient(options);
    await client.sync(snapshot(1));
    const edited = snapshot(1);
    edited.conversations[0].content_revision = 2;
    await client.sync(edited);
    const restored = new AgentClient(options);
    await restored.load();
    const removed = snapshot(1);
    removed.conversations[0].content_revision = 3;
    await restored.sync(removed);
    assert.deepEqual(revisions, [2, 3]);
  }));

test("long output splits Unicode safely and assigns stable part slots", async () =>
  fixture(async (statePath) => {
    const inputs = [];
    const client = new AgentClient({
      statePath,
      replayHistory: true,
      api: {
        request: async (path, options) => {
          if (path.startsWith("history")) return page([message(1)]);
          inputs.push(options.body);
          return { message: message(2, "bot-id"), replayed: false };
        },
      },
      onMessage: async (_message, { client }) => {
        assert.equal(client.taskId("ticket"), "room-id:m-1:ticket");
        await client.sendLong("🙂".repeat(8100), { slot: "report" });
      },
    });
    await client.sync(snapshot(1));
    assert.equal(inputs.length, 3);
    assert.ok(inputs.every((input) => Array.from(input.text).length <= 4000));
    assert.equal(
      inputs.map((input) => input.text.replace(/^\[\d+\/\d+\] /, "")).join(""),
      "🙂".repeat(8100),
    );
  }));

test("long output recovery retains the entire original batch when the model changes", async () =>
  fixture(async (statePath) => {
    let failed = false;
    const delivered = [];
    const api = {
      request: async (path, options) => {
        if (path.startsWith("history")) return page([message(1)]);
        const saved = JSON.parse(await readFile(statePath, "utf8"));
        assert.equal(Object.keys(saved.outbox).length, 3);
        if (!failed) {
          failed = true;
          throw new TypeError("lost response");
        }
        delivered.push(options.body.text);
        return { message: message(2, "bot-id"), replayed: false };
      },
    };
    const first = new AgentClient({
      api,
      statePath,
      replayHistory: true,
      onMessage: async (_m, { client }) => client.sendLong("🙂".repeat(8100)),
    });
    await assert.rejects(first.sync(snapshot(1)), TypeError);
    const restored = new AgentClient({
      api,
      statePath,
      onMessage: async (_m, { client }) => client.sendLong("short replacement"),
    });
    await restored.load();
    await restored.sync(snapshot(1));
    assert.equal(delivered.length, 3);
    assert.equal(
      delivered.map((text) => text.replace(/^\[\d+\/\d+\] /, "")).join(""),
      "🙂".repeat(8100),
    );
  }));
