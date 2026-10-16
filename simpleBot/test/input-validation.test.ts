import assert from "node:assert/strict";
import { test } from "node:test";
import { BotApi, BotApiError } from "../src/api.ts";
import { locateInputs } from "../src/input-validation.ts";
import type { TaskRecord } from "../src/store.ts";
import type { Message, HistoryResponse } from "../src/wire.ts";
import { MockServer } from "./mock-server.ts";

function task(message: Message): TaskRecord {
  return {
    conversation: "conv-1",
    input: message.id,
    sequence: message.sequence,
    promptHash: "hash",
    replyTo: null,
    status: "queued",
    attempts: 0,
    slots: [],
    updated: 0,
  };
}
function page(
  messages: Message[],
  revision = 1,
  hasAfter = false,
): HistoryResponse {
  return {
    conversation_id: "conv-1",
    messages,
    content_revision: revision,
    has_before: false,
    has_after: hasAfter,
  };
}
function fixture() {
  const server = new MockServer();
  const api = new BotApi({
    baseUrl: "http://localhost.invalid",
    token: "synthetic",
  });
  return { server, api, signal: new AbortController().signal };
}

test("one dense read returns current edited inputs and excludes retracted or mismatched IDs", async () => {
  const { server, api, signal } = fixture();
  const a = server.addMessage("old");
  const b = server.addMessage("removed");
  const c = server.addMessage("other");
  const tasks = [a, b, c].map(task);
  server.editMessage(a.id, "current version");
  server.retractMessage(b.id);
  let reads = 0;
  api.history = async (query) => {
    reads++;
    assert.equal(query.start, a.sequence);
    assert.equal(query.end, c.sequence);
    assert.equal(query.limit, 100);
    return page([
      a,
      { ...c, id: "unrelated" },
      { ...c, sequence: c.sequence + 10 },
    ]);
  };
  const result = await locateInputs(api, "conv-1", tasks, signal);
  assert.equal(reads, 1);
  assert.equal(result.requests, 1);
  assert.equal(result.messages.get(a.id)?.text, "current version");
  assert.deepEqual([...result.messages.keys()], [a.id]);
});

test("byte-limited validation pins following pages and restarts once on revision conflict", async () => {
  const { server, api, signal } = fixture();
  const a = server.addMessage("old");
  const b = server.addMessage("tail");
  let reads = 0;
  api.history = async (query) => {
    reads++;
    if (reads === 1) return page([{ ...a, text: "superseded" }], 1, true);
    if (reads === 2) {
      assert.equal(query.content_revision, 1);
      assert.equal(query.start, a.sequence + 1);
      throw new BotApiError(409, {
        code: "conflict",
        message: "revision changed",
        retryable: false,
      });
    }
    assert.equal(query.content_revision, undefined);
    assert.equal(query.start, a.sequence);
    return page([{ ...a, text: "current" }, b], 2);
  };
  const result = await locateInputs(api, "conv-1", [task(a), task(b)], signal);
  assert.equal(result.requests, 3);
  assert.equal(result.messages.get(a.id)?.text, "current");
  assert.equal(result.messages.get(b.id)?.text, "tail");
});

test("sparse validation is capped at four reads and abort starts no later groups", async () => {
  const { server, api } = fixture();
  const tasks = Array.from({ length: 9 }, (_, index) =>
    task({ ...server.addMessage("sparse"), sequence: 200 * (index + 1) }),
  );
  const gate = Promise.withResolvers<void>();
  const started = Promise.withResolvers<void>();
  const controller = new AbortController();
  let reads = 0;
  api.history = async (_query, signal) => {
    assert.equal(signal, controller.signal);
    reads++;
    if (reads === 4) started.resolve();
    await gate.promise;
    return page([]);
  };
  const running = locateInputs(api, "conv-1", tasks, controller.signal);
  await started.promise;
  assert.equal(reads, 4);
  controller.abort();
  gate.resolve();
  await assert.rejects(running, /abort/i);
  assert.equal(reads, 4);
});

test("a revision failure cannot return partial accepted inputs", async () => {
  const { server, api, signal } = fixture();
  const a = server.addMessage("a");
  const b = server.addMessage("b");
  api.history = async (query) => {
    if (query.content_revision === undefined) return page([a], 1, true);
    throw new BotApiError(409, {
      code: "conflict",
      message: "changed again",
      retryable: false,
    });
  };
  await assert.rejects(
    locateInputs(api, "conv-1", [task(a), task(b)], signal),
    /changed again/,
  );
});
