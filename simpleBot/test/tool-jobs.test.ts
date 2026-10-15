import assert from "node:assert/strict";
import { test } from "node:test";
import { setTimeout as wait } from "node:timers/promises";
import { ToolLocks } from "../src/tool-locks.ts";
import { ToolJobs } from "../src/tool-jobs.ts";
import {
  ModelClient,
  type ChatMessage,
  type FunctionTool,
} from "../src/model.ts";
import { toolAccess } from "../src/tools.ts";
import { BotApi } from "../src/api.ts";
import { Execution } from "../src/execution.ts";
import { Memory } from "../src/memory.ts";
import { Store } from "../src/store.ts";
import { createServer, type ServerResponse } from "node:http";
import { mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { compactConversation } from "../src/tokens.ts";

const tools: FunctionTool[] = ["memory", "execute_code", "finish_turn"].map(
  (name) => ({
    type: "function",
    function: { name, description: name, parameters: { type: "object" } },
  }),
);
const request = {
  backgroundTools: true,
  messages: [{ role: "user" as const, content: "synthetic request" }],
  tools,
  toolAccess: (name: string, args: string) =>
    toolAccess(name, args, "room", "workspace"),
};
const client = () =>
  new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    stream: false,
    timeoutMs: 3000,
    log: () => {},
  });
const call = (name: string, args = "{}", id = name) => ({
  id,
  type: "function",
  function: { name, arguments: args },
});
function response(
  content: string,
  calls: ReturnType<typeof call>[] = [],
  stream = false,
): Response {
  return stream
    ? new Response(
        `data: ${JSON.stringify({ choices: [{ delta: { content, tool_calls: calls.map((item, index) => ({ ...item, index })) }, finish_reason: calls.length ? "tool_calls" : "stop" }] })}\n\ndata: [DONE]\n\n`,
      )
    : Response.json({
        choices: [
          {
            message: { content, tool_calls: calls },
            finish_reason: calls.length ? "tool_calls" : "stop",
          },
        ],
      });
}

test("resource locks overlap readers, exclude writers and prevent writer starvation", async () => {
  const locks = new ToolLocks();
  const signal = new AbortController().signal;
  const readers = Promise.withResolvers<void>();
  const writer = Promise.withResolvers<void>();
  const writerStarted = Promise.withResolvers<void>();
  const bothStarted = Promise.withResolvers<void>();
  const events: string[] = [];
  const read = (id: string) =>
    locks.run({ resource: "memory", mode: "read" }, signal, async () => {
      events.push(id);
      if (events.length === 2) bothStarted.resolve();
      await readers.promise;
    });
  const a = read("reader-a"),
    b = read("reader-b");
  const w = locks.run(
    { resource: "memory", mode: "write" },
    signal,
    async () => {
      events.push("writer");
      writerStarted.resolve();
      await writer.promise;
    },
  );
  const late = locks.run(
    { resource: "memory", mode: "read" },
    signal,
    async () => {
      events.push("late-reader");
    },
  );
  await bothStarted.promise;
  await locks.run(
    { resource: "unrelated", mode: "write" },
    signal,
    async () => {
      events.push("unrelated");
    },
  );
  assert.deepEqual(events, ["reader-a", "reader-b", "unrelated"]);
  readers.resolve();
  await writerStarted.promise;
  assert.equal(events.at(-1), "writer");
  writer.resolve();
  await Promise.all([a, b, w, late]);
  assert.equal(events.at(-1), "late-reader");
});

test("cancelled queued writers never execute and failed writers release their lock", async () => {
  const locks = new ToolLocks();
  const gate = Promise.withResolvers<void>();
  const controller = new AbortController();
  const access = { resource: "workspace", mode: "write" as const };
  const first = locks.run(
    access,
    new AbortController().signal,
    () => gate.promise,
  );
  let executed = false;
  const queued = locks.run(access, controller.signal, async () => {
    executed = true;
  });
  controller.abort(new Error("cancelled"));
  await assert.rejects(queued, /cancelled/);
  gate.resolve();
  await first;
  assert.equal(executed, false);
  await assert.rejects(
    locks.run(access, new AbortController().signal, async () => {
      throw new Error("failure");
    }),
    /failure/,
  );
  await locks.run(access, new AbortController().signal, async () => {
    executed = true;
  });
  assert.equal(executed, true);
});

test("jobs share workspace exclusion across turns and collect failures once", async () => {
  const locks = new ToolLocks();
  const gate = Promise.withResolvers<void>();
  const signal = new AbortController().signal;
  const access = () => ({ resource: "workspace", mode: "write" as const });
  const first = new ToolJobs(
    async () => {
      await gate.promise;
      throw new Error("script failed");
    },
    access,
    locks,
    signal,
  );
  let secondRan = false;
  const second = new ToolJobs(
    async () => {
      secondRan = true;
      return "ok";
    },
    access,
    locks,
    signal,
  );
  try {
    assert.equal(
      JSON.parse(await first.dispatch("execute_code", "{}")).status,
      "running",
    );
    assert.equal(
      JSON.parse(await second.dispatch("execute_code", "{}")).status,
      "queued",
    );
    assert.equal(secondRan, false);
    await assert.rejects(second.wait('{"job_ids":["unknown"]}'), /job IDs/);
    gate.resolve();
    const failed = JSON.parse(await first.wait());
    assert.equal(failed.jobs[0].status, "failed");
    assert.match(failed.jobs[0].result, /script failed/);
    const done = JSON.parse(await second.wait());
    assert.equal(done.jobs[0].result, "ok");
    assert.deepEqual(first.collect(), []);
    assert.deepEqual(second.collect(), []);
  } finally {
    gate.resolve();
    await first.close();
    await second.close();
  }
});

for (const stream of [false, true])
  test(`model does independent work during a slow tool and explicitly waits for its result (stream=${stream})`, async (t) => {
    const gate = Promise.withResolvers<void>();
    const steps: string[] = [];
    const bodies: { messages: ChatMessage[]; tools: FunctionTool[] }[] = [];
    t.mock.method(
      globalThis,
      "fetch",
      async (_url: unknown, init: RequestInit) => {
        const body = JSON.parse(String(init.body));
        bodies.push(body);
        assert.ok(
          body.tools.some(
            (tool: FunctionTool) => tool.function.name === "wait_jobs",
          ),
        );
        if (bodies.length === 1)
          return response("", [call("execute_code")], stream);
        if (bodies.length === 2) {
          assert.equal(
            JSON.parse(body.messages.at(-1).content).job_id,
            "job-1",
          );
          assert.deepEqual(steps, ["script-started"]);
          return response(
            "",
            [call("memory", '{"action":"get","key":"index"}')],
            stream,
          );
        }
        if (bodies.length === 3) {
          assert.deepEqual(steps, ["script-started", "memory-read"]);
          assert.equal(body.messages.at(-1).content, "memory value");
          // The model reached a third inference before the script was released.
          gate.resolve();
          return response(
            "",
            [call("wait_jobs", '{"job_ids":["job-1"]}')],
            stream,
          );
        }
        assert.equal(bodies.length, 4);
        assert.match(
          body.messages
            .map((message: ChatMessage) => message.content)
            .join("\n"),
          /script-result/,
        );
        return response("verified answer", [], stream);
      },
    );
    const model = new ModelClient({
      endpoint: "https://model.invalid/",
      token: "",
      model: "test",
      stream,
      timeoutMs: 3000,
      log: () => {},
    });
    const answer = await model.complete({
      ...request,
      executeTool: async (name) => {
        if (name === "execute_code") {
          steps.push("script-started");
          await gate.promise;
          steps.push("script-finished");
          return "script-result";
        }
        steps.push("memory-read");
        return "memory value";
      },
    });
    assert.equal(answer, "verified answer");
    // Every provider tool call has exactly one correctly paired tool response.
    for (const body of bodies)
      for (const message of body.messages)
        for (const item of message.tool_calls ?? [])
          assert.equal(
            body.messages.filter((result) => result.tool_call_id === item.id)
              .length,
            1,
          );
  });

test("premature final drafts are withheld and regenerated after results, even at the tool limit", async (t) => {
  const gate = Promise.withResolvers<void>();
  let attempts = 0;
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init: RequestInit) => {
      attempts++;
      if (attempts === 1) return response("", [call("execute_code")]);
      if (attempts === 2) {
        gate.resolve();
        return response("unverified draft");
      }
      assert.equal(attempts, 3);
      const messages: ChatMessage[] = JSON.parse(String(init.body)).messages;
      assert.ok(
        !messages.some((message) => message.content === "unverified draft"),
      );
      assert.match(
        messages.at(-1)?.content ?? "",
        /tool_job_results.*verified/,
      );
      return response("corrected answer");
    },
  );
  assert.equal(
    await client().complete({
      ...request,
      maxToolCalls: 1,
      executeTool: async () => {
        await gate.promise;
        return "verified";
      },
    }),
    "corrected answer",
  );
});

test("completed failures are delivered automatically before the next model step without rerunning effects", async (t) => {
  const gate = Promise.withResolvers<void>();
  let attempts = 0,
    executions = 0;
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init: RequestInit) => {
      attempts++;
      if (attempts === 1) return response("", [call("execute_code")]);
      if (attempts === 2) {
        gate.resolve();
        return response("", [call("memory", '{"action":"get","key":"index"}')]);
      }
      assert.equal(attempts, 3);
      const messages: ChatMessage[] = JSON.parse(String(init.body)).messages;
      const notice = JSON.parse(messages.at(-1)?.content ?? "{}");
      assert.equal(notice.type, "tool_job_results");
      assert.equal(notice.jobs[0].status, "failed");
      assert.match(notice.jobs[0].result, /network failed; outcome uncertain/);
      return response("The operation failed; its outcome needs checking.");
    },
  );
  assert.match(
    await client().complete({
      ...request,
      executeTool: async (name) => {
        if (name === "memory") return "note";
        executions++;
        await gate.promise;
        throw new Error("network failed; outcome uncertain");
      },
    }),
    /outcome needs checking/,
  );
  assert.equal(executions, 1);
});

test("background result compaction preserves correlation, JSON and explicit truncation", () => {
  const notice = JSON.stringify({
    type: "tool_job_results",
    jobs: [
      {
        job_id: "job-1",
        tool: "execute_code",
        status: "completed",
        result: "x".repeat(20000),
      },
    ],
  });
  const messages: ChatMessage[] = [{ role: "user", content: notice }];
  const compact = compactConversation(messages, undefined, 500, 1 / 3);
  const decoded = JSON.parse(compact[0]?.content ?? "{}");
  assert.equal(decoded.jobs[0].job_id, "job-1");
  assert.equal(decoded.jobs[0].status, "completed");
  assert.match(decoded.jobs[0].result, /truncated/);
  assert.equal(messages[0]?.content, notice);
  assert.doesNotThrow(() =>
    compactConversation(
      [{ role: "user", content: '{"type":"tool_job_results",broken' }],
      undefined,
      1,
      1,
    ),
  );
});

test("finish_turn waits for observed results and prevents subsequent effects", async (t) => {
  const gate = Promise.withResolvers<void>();
  let attempts = 0,
    finished = false;
  const effects: string[] = [];
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init: RequestInit) => {
      attempts++;
      if (attempts === 1)
        return response("", [call("execute_code"), call("finish_turn")]);
      const messages: ChatMessage[] = JSON.parse(String(init.body)).messages;
      if (attempts === 2) {
        assert.equal(finished, false);
        assert.match(
          messages.at(-1)?.content ?? "",
          /reviewing all tool results/,
        );
        gate.resolve();
        return response("", [
          call("wait_jobs"),
          call("finish_turn", "{}", "too-early"),
        ]);
      }
      assert.equal(attempts, 3);
      assert.equal(
        finished,
        false,
        "a finish plan made before seeing wait results is rejected",
      );
      return response("", [
        call("finish_turn", "{}", "finish-2"),
        call("memory"),
      ]);
    },
  );
  assert.equal(
    await client().complete({
      ...request,
      stopAfterTool: () => finished,
      executeTool: async (name) => {
        effects.push(name);
        if (name === "execute_code") await gate.promise;
        if (name === "finish_turn") finished = true;
        return "ok";
      },
    }),
    "",
  );
  assert.deepEqual(effects, ["execute_code", "finish_turn"]);
});

test("model failure cancels active work, removes queued writes and waits for cleanup", async (t) => {
  let attempts = 0,
    cleaned = false;
  const effects: string[] = [];
  t.mock.method(globalThis, "fetch", async () => {
    if (++attempts === 1)
      return response("", [
        call("execute_code", "{}", "a"),
        call("execute_code", "{}", "b"),
      ]);
    return new Response("private upstream failure", { status: 401 });
  });
  await assert.rejects(
    client().complete({
      ...request,
      executeTool: async (_name, _args, signal) => {
        effects.push("started");
        try {
          await wait(10000, undefined, { signal });
        } finally {
          cleaned = true;
        }
        return "unexpected";
      },
    }),
    /HTTP 401/,
  );
  assert.equal(cleaned, true);
  assert.deepEqual(effects, ["started"]);
});

test("tool job timeout aborts cooperative work and the next completion can reuse its lock", async (t) => {
  let attempts = 0,
    cleaned = false;
  t.mock.method(globalThis, "fetch", async () => {
    attempts++;
    return attempts % 2 === 1
      ? response("", [call("execute_code")])
      : response("draft");
  });
  const model = new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    stream: false,
    timeoutMs: 100,
    log: () => {},
  });
  await assert.rejects(
    model.complete({
      ...request,
      executeTool: async (_name, _args, signal) => {
        try {
          await wait(10000, undefined, { signal });
        } finally {
          cleaned = true;
        }
        return "late";
      },
    }),
  );
  assert.equal(cleaned, true);
  assert.equal(
    await model.complete({ ...request, executeTool: async () => "quick" }),
    "draft",
  );
});

test("tool HTTP cancellation aborts fetch and suppresses retries", async (t) => {
  const controller = new AbortController();
  let attempts = 0;
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init: RequestInit) => {
      attempts++;
      controller.abort(new Error("turn cancelled"));
      init.signal?.throwIfAborted();
      throw new Error("unexpected");
    },
  );
  const api = new BotApi({
    baseUrl: "https://chat.invalid/",
    token: "synthetic",
  });
  await assert.rejects(
    api.executeCommand("room", "/whoami", "key", controller.signal),
    /turn cancelled/,
  );
  assert.equal(attempts, 1);
});

for (const cancel of [false, true])
  test(`real network subprocess frees the model for KV reads and cleans up (cancel=${cancel})`, async (t) => {
    const dir = await mkdtemp(join(tmpdir(), "simplebot-jobs-"));
    const execution = new Execution(join(dir, "workspace"));
    const store = await Store.open(":memory:");
    const memory = new Memory(store.records.kv, "bot", "room");
    const requested = Promise.withResolvers<void>();
    const controller = new AbortController();
    let pending: ServerResponse | undefined;
    const server = createServer((_request, reply) => {
      pending = reply;
      requested.resolve();
    });
    let attempts = 0,
      reads = 0,
      scripts = 0;
    try {
      await new Promise<void>((resolve) =>
        server.listen(0, "127.0.0.1", resolve),
      );
      const address = server.address();
      assert.ok(address && typeof address !== "string");
      await memory.execute(
        '{"action":"set","key":"index","value":"remembered"}',
      );
      const code = `const response = await fetch("http://127.0.0.1:${address.port}/"); console.log(await response.text()); await Deno.writeTextFile("result.txt", "saved");`;
      t.mock.method(
        globalThis,
        "fetch",
        async (_url: unknown, init: RequestInit) => {
          attempts++;
          if (attempts === 1)
            return response("", [
              call("execute_code", JSON.stringify({ code })),
            ]);
          if (attempts === 2) {
            // The tool's actual HTTP request is waiting while inference runs.
            await requested.promise;
            assert.ok(pending && !pending.writableEnded);
            return response("", [
              call("memory", '{"action":"get","key":"index"}'),
            ]);
          }
          if (attempts === 3) {
            assert.equal(reads, 1);
            assert.ok(pending && !pending.writableEnded);
            const messages: ChatMessage[] = JSON.parse(
              String(init.body),
            ).messages;
            assert.match(messages.at(-1)?.content ?? "", /remembered/);
            if (cancel) controller.abort(new Error("stop turn"));
            else pending.end("network-result");
            return response("", [call("wait_jobs")]);
          }
          assert.equal(attempts, 4);
          const messages: ChatMessage[] = JSON.parse(
            String(init.body),
          ).messages;
          assert.match(messages.at(-1)?.content ?? "", /network-result/);
          return response("completed");
        },
      );
      const work = client().complete({
        ...request,
        signal: controller.signal,
        executeTool: async (name, args, signal) => {
          if (name === "memory") {
            reads++;
            return memory.execute(args);
          }
          scripts++;
          assert.ok(signal);
          return execution.execute(args, signal);
        },
      });
      if (cancel) await assert.rejects(work, /stop turn/);
      else {
        assert.equal(await work, "completed");
        assert.equal(
          await readFile(join(execution.workspace, "result.txt"), "utf8"),
          "saved",
        );
      }
      await execution.settle();
      assert.equal(scripts, 1);
      assert.deepEqual(await readdir(dir), ["workspace"]);
      if (cancel) assert.deepEqual(await readdir(execution.workspace), []);
    } finally {
      controller.abort();
      pending?.destroy();
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
      await execution.settle();
      await store.close();
      await rm(dir, { recursive: true, force: true });
    }
  });
