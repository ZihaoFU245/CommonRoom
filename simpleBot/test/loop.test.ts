import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "../src/agent.ts";
import { BotApi } from "../src/api.ts";
import { ConfigStore } from "../src/config.ts";
import { Store, textHash } from "../src/store.ts";
import type { CompletionRequest, ChatModel } from "../src/model.ts";
import { ModelClient, ModelError } from "../src/model.ts";
import { tickDecision, type TickDecision } from "../src/tick.ts";
import { MEMORY_TOOL } from "../src/memory.ts";
import { readJob } from "../src/job-results.ts";
import { MockServer } from "./mock-server.ts";

const wait: TickDecision = {
  messages: [],
  actions: [],
  next: { kind: "wait" },
};

async function until(check: () => boolean): Promise<void> {
  const deadline = Date.now() + 4000;
  while (!check()) {
    if (Date.now() > deadline) throw new Error("Event did not arrive");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

async function fixture(
  tick: (request: CompletionRequest, call: number) => Promise<TickDecision>,
  withShell = false,
) {
  const server = new MockServer();
  const url = await server.listen();
  const dir = mkdtempSync(join(tmpdir(), "simplebot-loop-"));
  const config = ConfigStore.load(join(dir, "config.json"));
  config.set("chat.historyWindow", 0);
  let store = await Store.open(join(dir, "agent.kv"));
  const requests: CompletionRequest[] = [];
  const model: ChatModel = {
    describe: "tick-model",
    complete: () => {
      throw new Error("Hidden completion loop used");
    },
    tick: (request) => {
      requests.push(request);
      return tick(request, requests.length);
    },
  };
  const api = new BotApi({ baseUrl: url, token: server.token });
  const logs: string[] = [];
  const create = () =>
    new Agent({
      api,
      store,
      config,
      model,
      workspace: join(dir, "workspace"),
      withShell,
      log: (_level, text) => logs.push(text),
      presence: () => [],
      socketReady: () => true,
      setActivity: () => {},
    });
  let agent = create();
  await agent.handleSnapshot(server.snapshot());
  return {
    server,
    config,
    api,
    requests,
    logs,
    dir,
    get agent() {
      return agent;
    },
    get store() {
      return store;
    },
    async restart(nextShell = withShell) {
      await agent.stop();
      withShell = nextShell;
      await store.close();
      store = await Store.open(join(dir, "agent.kv"));
      agent = create();
      await agent.handleSnapshot(server.snapshot());
    },
    async close() {
      await agent.stop();
      await store.close();
      await server.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

test("dense group input revalidation uses one range read and one model step", async () => {
  const h = await fixture(async (request) => {
    request.onCall?.({
      success: true,
      durationMs: 10,
      usage: { prompt: 100, completion: 1 },
    });
    return { ...wait, messages: [{ text: "combined answer" }] };
  });
  try {
    h.config.set("autonomous", true);
    h.config.set("chat.batchWindowMs", 0);
    for (let index = 0; index < 20; index++)
      h.server.addMessage(`input ${index}`, { mentions: [] });
    await h.agent.requestSync();
    assert.equal(h.requests.length, 1);
    assert.equal(h.server.state.sends.length, 1);
    assert.equal(h.agent.metrics("conv-1").validationInputs, 20);
    assert.equal(h.agent.metrics("conv-1").validationRequests, 1);
    assert.match(
      h.agent.metrics("conv-1").report(),
      /End-to-end reply \| [^|]+ \| [^|]+ \| [^|]+ \| 20 \|/,
    );
    assert.match(
      h.agent.metrics("conv-1").report(),
      /Model calls per answered input \| 1\.00/,
    );
  } finally {
    await h.close();
  }
});

test("one step batches independent lookups and waits once for all results", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1
      ? {
          messages: [],
          actions: ["a", "b", "c"].map((id) => ({
            id,
            tool: "memory",
            arguments: { action: "get", key: id },
          })),
          next: { kind: "on_jobs", jobs: ["a", "b", "c"], mode: "all" },
        }
      : { ...wait, messages: [{ text: "three results" }] },
  );
  try {
    h.server.addMessage("@helper read three documents");
    await h.agent.requestSync();
    await until(() => h.server.state.sends.length === 1);
    assert.equal(h.requests.length, 2);
    assert.equal(
      Object.values(h.store.state.sessions["conv-1"].jobs).filter(
        (job) => job.status === "completed",
      ).length,
      3,
    );
    assert.equal(h.server.state.sends[0].text, "three results");
  } finally {
    await h.close();
  }
});

test("autonomous windows are fixed, configurable, and explicit mentions bypass them", async () => {
  const h = await fixture(async () => wait);
  try {
    h.config.set("autonomous", true);
    h.config.set("chat.batchWindowMs", 10000);
    const first = h.server.addMessage("background discussion", {
      mentions: [],
    });
    await h.agent.requestSync(true, false);
    const deadline = h.store.task("conv-1", first.id)?.readyAt;
    assert.ok((deadline ?? 0) >= Date.now() + 9000);
    h.config.set("chat.batchWindowMs", 250);
    const later = h.server.addMessage("same window", { mentions: [] });
    await h.agent.requestSync(true, false);
    assert.equal(h.store.task("conv-1", later.id)?.readyAt, deadline);
    assert.equal(h.requests.length, 0);
    h.server.addMessage("@helper answer now");
    await h.agent.requestSync(true, false);
    await until(() => h.requests.length === 1);
    assert.ok(h.requests[0].messages.at(-1)?.content.includes("answer now"));
  } finally {
    await h.close();
  }
});

test("grant refreshes keep the instruction prefix stable and update metadata each tick", async () => {
  const h = await fixture(async () => wait);
  let reads = 0;
  h.api.serverCommands = async () => ({
    conversation_id: h.server.conversationId,
    commands: [
      {
        name: `/command-${++reads}`,
        usage: `/command-${reads}`,
        description: "Synthetic command",
      },
    ],
  });
  try {
    for (const text of ["first", "second"]) {
      h.server.addMessage(`@helper ${text}`);
      await h.agent.requestSync();
      await until(() => h.requests.length === (text === "first" ? 1 : 2));
    }
    assert.equal(reads, 2);
    assert.equal(
      h.requests[0].messages[0].content,
      h.requests[1].messages[0].content,
    );
    assert.ok(!h.requests[0].messages[0].content.includes("/command-1"));
    assert.ok(
      h.requests[0].messages.some((message) =>
        message.content.includes("Server command grants: /command-1"),
      ),
    );
    assert.ok(
      h.requests[1].messages.some((message) =>
        message.content.includes("Server command grants: /command-2"),
      ),
    );
    assert.ok(
      !h.requests[1].messages.some((message) =>
        message.content.includes("Server command grants: /command-1"),
      ),
    );
  } finally {
    await h.close();
  }
});

test("live reply limits reject jobs atomically before any effect", async () => {
  const h = await fixture(async (request, call) => {
    if (call !== 1) return wait;
    request.maxReplyMessages = 0;
    return {
      messages: [{ text: "must not send" }],
      actions: [
        {
          id: "write",
          tool: "memory",
          arguments: { action: "set", key: "index", value: "must not save" },
        },
      ],
      next: { kind: "wait" },
    };
  });
  try {
    h.server.addMessage("@helper bounded reply");
    await h.agent.requestSync();
    await until(() => h.requests.length === 2);
    assert.equal(h.server.state.sends.length, 0);
    assert.equal(Object.keys(h.store.state.sessions["conv-1"].jobs).length, 0);
    assert.ok(
      h.logs.some((line) =>
        line.includes("Reply plan exceeds this tick's message limit"),
      ),
    );
  } finally {
    await h.close();
  }
});

test("context preparation overlaps live catalogue and retained-history reads", async () => {
  const h = await fixture(async () => wait);
  const gate = Promise.withResolvers<void>();
  let catalogueStarted = false;
  let historyStarted = false;
  let syncing: Promise<void> | undefined;
  const catalogue = h.api.serverCommands.bind(h.api);
  const history = h.api.history.bind(h.api);
  h.api.serverCommands = async (...args) => {
    catalogueStarted = true;
    await gate.promise;
    return catalogue(...args);
  };
  h.api.history = async (query, signal) => {
    if (query.before !== undefined) {
      historyStarted = true;
      await gate.promise;
    }
    return history(query, signal);
  };
  try {
    h.config.set("chat.historyWindow", 5);
    h.server.addMessage("@helper prepare context");
    syncing = h.agent.requestSync();
    await until(() => catalogueStarted && historyStarted);
    assert.equal(h.requests.length, 0);
    gate.resolve();
    await syncing;
    await until(() => h.requests.length === 1);
    assert.ok(
      h.logs.some((line) =>
        /model preparation finished.*duration_ms=\d+/.test(line),
      ),
    );
  } finally {
    gate.resolve();
    await syncing;
    await h.close();
  }
});

for (const stream of [false, true])
  test(`mixed provider response executes one durable job and sends one reply (stream=${stream})`, async (t) => {
    const fetch = globalThis.fetch;
    let providerCalls = 0;
    let executions = 0;
    const action = {
      id: "保存:index",
      tool: "memory",
      arguments: { action: "set", key: "index", value: "remembered" },
    };
    t.mock.method(
      globalThis,
      "fetch",
      async (input: string | URL | Request, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : String(input);
        if (!url.startsWith("https://model.invalid/"))
          return fetch(input, init);
        providerCalls++;
        const calls =
          providerCalls === 1
            ? [
                {
                  id: "native-call",
                  type: "function",
                  function: {
                    name: "memory",
                    arguments: JSON.stringify(action.arguments),
                  },
                },
                {
                  id: "envelope-call",
                  type: "function",
                  function: {
                    name: "finish_tick",
                    arguments: JSON.stringify({
                      messages: [],
                      actions: [action],
                      next: { kind: "on_jobs", jobs: [action.id], mode: "all" },
                    }),
                  },
                },
              ]
            : [];
        const content = providerCalls === 1 ? "provisional" : "saved once";
        return stream
          ? new Response(
              `data: ${JSON.stringify({ choices: [{ delta: { content, tool_calls: calls.map((call, index) => ({ index, ...call })) }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`,
            )
          : Response.json({
              choices: [
                {
                  message: { content, tool_calls: calls },
                  finish_reason: "stop",
                },
              ],
            });
      },
    );
    const model = new ModelClient({
      endpoint: "https://model.invalid/v1",
      model: "test",
      token: "",
      stream,
      timeoutMs: 1000,
      log: () => {},
    });
    const h = await fixture(async (request) => {
      const execute = request.executeTool;
      request.executeTool = async (...args) => {
        executions++;
        assert.equal(
          Object.values(h.store.state.sessions["conv-1"].jobs).filter(
            (job) => job.status === "running",
          ).length,
          1,
        );
        assert.ok(execute);
        return execute(...args);
      };
      return await model.tick(request);
    });
    try {
      h.server.addMessage("@helper remember this");
      await h.agent.requestSync();
      await until(() => h.server.state.sends.length === 1);
      assert.equal(providerCalls, 2);
      assert.equal(executions, 1);
      assert.equal(
        Object.keys(h.store.state.sessions["conv-1"].jobs).length,
        1,
      );
      assert.deepEqual(
        h.server.state.sends.map((send) => send.text),
        ["saved once"],
      );
      await h.restart();
      assert.equal(providerCalls, 2);
      assert.equal(executions, 1);
      assert.equal(h.server.state.sends.length, 1);
    } finally {
      await h.close();
    }
  });

for (const stream of [false, true])
  test(`an action with an unknown field is normalized and still runs once (stream=${stream})`, async (t) => {
    const fetch = globalThis.fetch;
    let calls = 0;
    let executions = 0;
    t.mock.method(
      globalThis,
      "fetch",
      async (input: string | URL | Request, init?: RequestInit) => {
        const url = input instanceof Request ? input.url : String(input);
        if (!url.startsWith("https://model.invalid/"))
          return fetch(input, init);
        calls++;
        const body = JSON.parse(String(init?.body));
        // One envelope only: tools are advertised inside finish_tick.actions so
        // a tick can batch several actions instead of one call each.
        assert.deepEqual(
          body.tools.map(
            (tool: { function: { name: string } }) => tool.function.name,
          ),
          ["finish_tick"],
        );
        const tools = [
          calls === 1
            ? {
                id: "call-1",
                type: "function",
                function: {
                  name: "memory",
                  arguments: JSON.stringify({
                    action: "get",
                    key: "index",
                    language: "typescript",
                  }),
                },
              }
            : {
                id: "call-2",
                type: "function",
                function: {
                  name: "finish_tick",
                  arguments: JSON.stringify({
                    messages: [{ text: "normalized and done" }],
                    actions: [],
                    next: { kind: "wait" },
                  }),
                },
              },
        ];
        const content = calls === 1 ? "provisional" : null;
        return stream
          ? new Response(
              `data: ${JSON.stringify({ choices: [{ delta: { content, tool_calls: tools.map((call, index) => ({ index, ...call })) }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`,
            )
          : Response.json({
              choices: [
                {
                  message: { content, tool_calls: tools },
                  finish_reason: "stop",
                },
              ],
            });
      },
    );
    const modelLogs: string[] = [];
    const model = new ModelClient({
      endpoint: "https://model.invalid/v1",
      model: "test",
      token: "",
      stream,
      timeoutMs: 1000,
      log: (level, message) => modelLogs.push(`${level} ${message}`),
    });
    const h = await fixture(async (request) => {
      const execute = request.executeTool;
      if (execute)
        request.executeTool = async (...args) => {
          executions++;
          return execute(...args);
        };
      return await model.tick(request);
    });
    try {
      h.server.addMessage("@helper read the index");
      await h.agent.requestSync();
      await until(() => h.server.state.sends.length === 1);
      // One normalization, one real effect, no repair call in between.
      assert.equal(calls, 2);
      assert.equal(executions, 1);
      assert.equal(h.store.state.sessions["conv-1"].phase, "idle");
      assert.ok(
        modelLogs.some(
          (line) =>
            line.includes("model tick arguments normalized") &&
            line.includes('labels=["memory"]'),
        ),
      );
      assert.ok(!h.logs.some((line) => line.includes("tick retry scheduled")));
      assert.ok(
        modelLogs.some(
          (line) =>
            line.includes("model tick accepted") &&
            line.includes("format=native_tools") &&
            line.includes("normalized=1"),
        ),
      );
      assert.deepEqual(
        h.server.state.sends.map((send) => send.text),
        ["normalized and done"],
      );
      await h.restart();
      assert.equal(calls, 2);
      assert.equal(h.server.state.sends.length, 1);
    } finally {
      await h.close();
    }
  });

for (const autonomous of [false, true])
  test(`new input cannot steal a repairing input or inherit its exhausted retries (autonomous=${autonomous})`, async () => {
    const h = await fixture(async (request, call) => {
      if (call <= 3) {
        assert.ok(
          request.messages.at(-1)?.content.includes("original request"),
        );
        throw new ModelError(422, "Invalid tool arguments");
      }
      if (call === 4) {
        assert.equal(request.answerOnly, true);
        assert.deepEqual(request.tools, []);
        assert.equal(request.executeTool, undefined);
        return { ...wait, messages: [{ text: "original recovered" }] };
      }
      assert.equal(call, 5);
      assert.equal(request.answerOnly, false);
      assert.ok(request.messages.at(-1)?.content.includes("new request"));
      return { ...wait, messages: [{ text: "new answered" }] };
    });
    try {
      h.config.set("autonomous", autonomous);
      const first = h.server.addMessage("@helper original request");
      await h.agent.requestSync();
      const second = h.server.addMessage("@helper new request");
      await h.agent.requestSync();
      assert.equal(h.requests.length, 1);
      assert.equal(h.store.task("conv-1", second.id)?.status, "queued");
      await until(() => h.server.state.sends.length === 2);
      assert.equal(
        h.store.task("conv-1", first.id)?.replyHash,
        textHash(first.text),
      );
      assert.equal(
        h.store.task("conv-1", second.id)?.replyHash,
        textHash(second.text),
      );
      assert.equal(h.requests.length, 5);
      await h.restart();
      assert.equal(h.requests.length, 5);
    } finally {
      await h.close();
    }
  });

test("failed recovery cannot execute jobs or block later messages", async () => {
  const h = await fixture(async (request, call) => {
    if (call <= 3) throw new ModelError(422, "Invalid tool arguments");
    if (call === 4) {
      assert.equal(request.answerOnly, true);
      return {
        ...wait,
        actions: [
          {
            id: "must-not-run",
            tool: "memory",
            arguments: { action: "set", key: "index", value: "bad" },
          },
        ],
      };
    }
    assert.equal(call, 5);
    assert.equal(request.answerOnly, false);
    return { ...wait, messages: [{ text: "still responding" }] };
  });
  try {
    const first = h.server.addMessage("@helper failing request");
    await h.agent.requestSync();
    await until(() => h.store.task("conv-1", first.id)?.status === "failed");
    assert.equal(h.requests.length, 4);
    assert.equal(Object.keys(h.store.state.sessions["conv-1"].jobs).length, 0);
    h.server.addMessage("@helper fresh request");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 5);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["still responding"],
    );
  } finally {
    await h.close();
  }
});

test("a terminal model failure still spends one bounded tool-less recovery tick", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1)
      throw new ModelError(403, "model provider refused response");
    assert.equal(call, 2);
    assert.equal(request.answerOnly, true);
    assert.deepEqual(request.tools, []);
    assert.equal(request.executeTool, undefined);
    return {
      ...wait,
      messages: [{ text: "I couldn't complete that request." }],
    };
  });
  try {
    const input = h.server.addMessage("@helper refused request");
    await h.agent.requestSync();
    await until(() => h.server.state.sends.length === 1);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["I couldn't complete that request."],
    );
    assert.equal(h.store.task("conv-1", input.id)?.status, "answered");
    assert.equal(
      h.store.task("conv-1", input.id)?.replyHash,
      textHash(input.text),
    );
    assert.equal(h.requests.length, 2);
    assert.ok(h.logs.some((line) => line.includes("tick recovery scheduled")));
    assert.ok(!h.logs.some((line) => line.includes("tick failed")));
    await h.restart();
    assert.equal(h.requests.length, 2);
  } finally {
    await h.close();
  }
});

test("recovery is spent once: a second refusal fails visibly without another call", async () => {
  const h = await fixture(async (request, call) => {
    assert.ok(call <= 2);
    if (call === 2) assert.equal(request.answerOnly, true);
    throw new ModelError(403, "model provider refused response");
  });
  try {
    const input = h.server.addMessage("@helper refused request");
    await h.agent.requestSync();
    await until(() => h.store.task("conv-1", input.id)?.status === "failed");
    assert.equal(h.requests.length, 2);
    assert.equal(h.server.state.sends.length, 0);
    assert.ok(h.logs.some((line) => line.includes("tick failed")));
    await h.restart();
    await h.agent.requestSync();
    assert.equal(h.requests.length, 2);
  } finally {
    await h.close();
  }
});

test("an exhausted tool budget still spends one tool-less tick to answer", async () => {
  const h = await fixture(async (request, call) => {
    if (call <= 2)
      return {
        ...wait,
        actions: [
          {
            id: `note-${call}`,
            tool: "memory",
            arguments: { action: "set", key: "index", value: call },
          },
        ],
        next: { kind: "on_jobs", jobs: [`note-${call}`], mode: "all" },
      };
    assert.equal(call, 3);
    assert.equal(request.answerOnly, true);
    assert.deepEqual(request.tools, []);
    return { ...wait, messages: [{ text: "Here is the answer." }] };
  });
  try {
    h.config.set("model.maxToolCalls", 1);
    const input = h.server.addMessage("@helper do the work");
    await h.agent.requestSync();
    await until(() => h.server.state.sends.length === 1);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["Here is the answer."],
    );
    assert.equal(h.store.task("conv-1", input.id)?.status, "answered");
    assert.equal(h.requests.length, 3);
    assert.ok(h.logs.some((line) => line.includes("tick recovery scheduled")));
  } finally {
    await h.close();
  }
});

test("an exhausted episode tool budget publishes the answer instead of failing the input", async () => {
  const h = await fixture(async (request, call) => {
    assert.equal(call, 1);
    assert.ok(request.tools?.some((tool) => tool.function.name === "memory"));
    return {
      ...wait,
      messages: [{ text: "answered before the budget stopped the episode" }],
      actions: [
        {
          id: "kept",
          tool: "memory",
          arguments: { action: "set", key: "kept", value: 1 },
        },
        {
          id: "dropped",
          tool: "memory",
          arguments: { action: "set", key: "dropped", value: 2 },
        },
      ],
      next: { kind: "on_jobs", jobs: ["kept", "dropped"], mode: "all" },
    };
  });
  try {
    h.config.set("model.maxToolCalls", 1);
    const input = h.server.addMessage("@helper answer now");
    await h.agent.requestSync();
    await until(() => h.server.state.sends.length === 1);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["answered before the budget stopped the episode"],
    );
    assert.equal(h.store.task("conv-1", input.id)?.status, "answered");
    const session = h.store.state.sessions["conv-1"];
    assert.equal(session.phase, "idle");
    assert.deepEqual(session.next, { kind: "wait" });
    // Only the action that fitted the budget became a durable job.
    assert.equal(Object.keys(session.jobs).length, 1);
    assert.ok(
      h.logs.some((line) => line.includes("episode tool budget reached")),
    );
    assert.ok(!h.logs.some((line) => line.includes("tick failed")));
    assert.equal(h.requests.length, 1);
  } finally {
    await h.close();
  }
});

test("each continue is another single-call tick; silence and duplicate events do not restart it", async () => {
  const h = await fixture(async (_request, call) =>
    call < 3
      ? { ...wait, next: { kind: "continue" } }
      : { ...wait, messages: [{ text: "done" }] },
  );
  try {
    h.server.addMessage("@helper work");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 3);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["done"],
    );
    await h.agent.requestSync();
    await h.restart();
    assert.equal(h.requests.length, 3);
    assert.match(
      h.requests[0].messages[0].content,
      /Every tick is one model call/,
    );
  } finally {
    await h.close();
  }
});

test("a final answer without outstanding work does not schedule a second answer", async () => {
  const h = await fixture(async (_request, call) => ({
    ...wait,
    messages: [
      { text: call === 1 ? "first answer" : "different second answer" },
    ],
    next: call === 1 ? { kind: "continue" } : { kind: "wait" },
  }));
  try {
    h.server.addMessage("@helper one question");
    await h.agent.requestSync();
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["first answer"],
    );
    assert.equal(h.requests.length, 1);
    await h.restart();
    assert.equal(h.requests.length, 1);
  } finally {
    await h.close();
  }
});

test("two messages for one input in a single tick form one response, matching the reported screenshot", async () => {
  let input = "";
  const h = await fixture(async () => ({
    ...wait,
    messages: [
      { text: "收到，我在。", replyTo: input },
      { text: "在呢，你测吧。" },
    ],
  }));
  try {
    input = h.server.addMessage("@helper A test").id;
    await h.agent.requestSync();
    assert.equal(h.requests.length, 1);
    assert.deepEqual(
      h.server.state.sends.map((send) => [send.text, send.replyTo]),
      [["收到，我在。\n\n在呢，你测吧。", input]],
    );
    assert.ok(
      h.logs.some(
        (line) =>
          line.includes("tick=1 next=wait") &&
          line.includes(`inputs=${input} messages=1 jobs=0`),
      ),
    );
  } finally {
    await h.close();
  }
});

for (const targeted of [false, true])
  test(`a plain group response and its other parts form one message for a multi-input batch (targeted=${targeted})`, async () => {
    let first = "";
    const h = await fixture(async () => ({
      ...wait,
      messages: [
        { text: "first part", ...(targeted ? { replyTo: first } : {}) },
        { text: "second part" },
      ],
    }));
    try {
      h.config.set("autonomous", true);
      const a = h.server.addMessage("first input");
      first = a.id;
      const b = h.server.addMessage("second input", {
        authorId: "acct-bob",
        from: "bob",
      });
      await h.agent.requestSync();
      assert.equal(h.requests.length, 1);
      assert.deepEqual(
        h.server.state.sends.map((send) => [send.text, send.replyTo]),
        [["first part\n\nsecond part", null]],
      );
      assert.equal(h.store.task("conv-1", a.id)?.replyHash, textHash(a.text));
      assert.equal(h.store.task("conv-1", b.id)?.replyHash, textHash(b.text));
      assert.ok(
        h.logs.some(
          (line) =>
            line.includes("reply delivered") && line.includes("reply_to=none"),
        ),
      );
      assert.ok(
        h.logs.some(
          (line) =>
            line.includes("tick replies committed") &&
            line.includes(a.id) &&
            line.includes(b.id),
        ),
      );
      assert.ok(!h.logs.join("\n").includes("first part"));
      await h.restart();
      assert.equal(h.server.state.sends.length, 1);
    } finally {
      await h.close();
    }
  });

test("a watched job cannot produce another final answer for an already answered input", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1
      ? {
          messages: [{ text: "first answer" }],
          actions: [
            {
              id: "save",
              tool: "memory",
              arguments: { action: "set", key: "index", value: "note" },
            },
          ],
          next: { kind: "on_jobs", jobs: ["save"], mode: "all" },
        }
      : { ...wait, messages: [{ text: "different second answer" }] },
  );
  try {
    h.server.addMessage("@helper one question");
    await h.agent.requestSync();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["first answer"],
    );
    await h.restart();
    assert.equal(h.server.state.sends.length, 1);
  } finally {
    await h.close();
  }
});

for (const scenario of ["reply", "silent", "inflight", "v2"])
  test(`old group recovery retains committed output and unfinished batch members (${scenario})`, async () => {
    const silent = scenario === "silent";
    const inflight = scenario === "inflight";
    const modern = scenario === "v2";
    const h = await fixture(async () => {
      if (inflight)
        return { ...wait, messages: [{ text: "fresh group answer" }] };
      throw new Error("Committed legacy decision must not call the model");
    });
    try {
      h.config.set("autonomous", true);
      const first = h.server.addMessage("first question");
      const second = h.server.addMessage("second question");
      const batch = [first, second].map((message) => ({
        input: message.id,
        sequence: message.sequence,
        hash: textHash(message.text),
      }));
      for (const message of [first, second])
        h.store.putTask({
          conversation: "conv-1",
          input: message.id,
          sequence: message.sequence,
          promptHash: textHash(message.text),
          replyTo: message.id,
          status: modern ? "answered" : "running",
          attempts: 1,
          autonomous: true,
          readyAt: Date.now(),
          slots: [],
          updated: Date.now(),
          ...(message === first
            ? {
                batch,
                outputSlot: modern ? "answer" : "group:0",
                ...(modern ? { outputHash: textHash(message.text) } : {}),
                ...(inflight
                  ? {}
                  : { decision: silent ? [] : [{ text: "legacy answer" }] }),
              }
            : { batchOwner: first.id }),
        });
      const family = modern ? "answer" : "group:0:0";
      const slot = `conv-1:${first.id}:${family}`;
      if (!silent && !inflight) {
        const request = {
          conversation_id: "conv-1",
          text: "legacy answer",
          markdown: true,
        };
        await h.api.send(request, "original-legacy-send-key");
        h.store.task("conv-1", first.id)!.slots = [family];
        h.store.putPart({
          slot,
          key: "original-legacy-send-key",
          request,
          created: Date.now(),
        });
      }
      if (modern)
        h.store.state.sessions["conv-1"] = {
          root: first.id,
          generation: 0,
          tick: 1,
          calls: 1,
          started: Date.now(),
          phase: "idle",
          next: { kind: "wait" },
          jobs: {},
          sources: batch,
          trace: [{ tick: 1, messages: ["legacy answer"], actions: [] }],
        };
      h.store.state.version = modern ? 2 : 1;
      await h.store.save();
      await h.restart();
      assert.equal(h.requests.length, inflight ? 1 : 0);
      assert.deepEqual(
        h.server.state.sends.map((send) => send.text),
        silent ? [] : inflight ? ["fresh group answer"] : ["legacy answer"],
      );
      assert.equal(h.store.task("conv-1", second.id)?.status, "answered");
      if (!silent && !inflight)
        assert.equal(h.store.part(slot)?.response?.replayed, true);
      if (inflight)
        assert.ok(
          h.requests[0].messages.some((message) =>
            message.content.includes(second.id),
          ),
        );
      await h.restart();
      assert.equal(h.requests.length, inflight ? 1 : 0);
    } finally {
      await h.close();
    }
  });

test("fast tool completion wakes on_jobs once and survives a tick boundary", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1)
      return {
        ...wait,
        actions: [
          {
            id: "save",
            tool: "memory",
            arguments: { action: "set", key: "index", value: "remembered" },
          },
        ],
        next: { kind: "on_jobs", jobs: ["save"], mode: "all" },
      };
    assert.ok(
      request.messages.some((message) =>
        message.content.includes('"status":"completed"'),
      ),
    );
    return { ...wait, messages: [{ text: "saved" }] };
  });
  try {
    h.server.addMessage("@helper remember");
    await h.agent.requestSync();
    await until(() => h.server.state.sends.length === 1);
    assert.equal(h.requests.length, 2);
    assert.equal(
      Object.values(h.store.state.sessions["conv-1"].jobs)[0].observed,
      true,
    );
    await h.restart();
    assert.equal(h.requests.length, 2);
  } finally {
    await h.close();
  }
});

test("rewatching a result delivered in this tick does not tick again, including after restart", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1)
      return {
        ...wait,
        actions: [
          {
            id: "查找:index",
            tool: "memory",
            arguments: { action: "get", key: "index" },
          },
        ],
        next: { kind: "on_jobs", jobs: ["查找:index"], mode: "all" },
      };
    const state = JSON.parse(
      request.messages
        .find((message) => message.content.startsWith("Tick state"))!
        .content.split("\n")
        .slice(1)
        .join("\n"),
    );
    assert.equal(call, 2);
    return {
      ...wait,
      messages: [{ text: "done" }],
      next: { kind: "on_jobs", jobs: [state.jobs[0].id], mode: "any" },
    };
  });
  try {
    h.server.addMessage("@helper lookup");
    await h.agent.requestSync();
    await until(() =>
      h.logs.some((line) => line.includes("tick watch consumed")),
    );
    await h.agent.requestSync();
    assert.equal(h.store.state.sessions["conv-1"].next.kind, "wait");
    assert.equal(h.requests.length, 2);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["done"],
    );
    assert.ok(
      h.logs.some(
        (line) => line.includes("job started") && line.includes("tool=memory"),
      ),
    );
    assert.ok(
      h.logs.some(
        (line) =>
          line.includes("job finished") && line.includes("status=completed"),
      ),
    );
    assert.ok(
      h.logs.some(
        (line) =>
          line.includes("tick wake") && line.includes("reason=job_results"),
      ),
    );
    await h.restart();
    assert.equal(h.requests.length, 2);
  } finally {
    await h.close();
  }
});

test("a consumed watch cannot silently abandon an unanswered input and repair receives the reason", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1)
      return {
        ...wait,
        actions: [
          {
            id: "lookup",
            tool: "memory",
            arguments: { action: "get", key: "index" },
          },
        ],
        next: { kind: "on_jobs", jobs: ["lookup"], mode: "all" },
      };
    const state = JSON.parse(
      request.messages
        .find((message) => message.content.startsWith("Tick state"))!
        .content.split("\n")
        .slice(1)
        .join("\n"),
    );
    if (call === 2)
      return {
        ...wait,
        next: { kind: "on_jobs", jobs: [state.jobs[0].id], mode: "all" },
      };
    assert.equal(call, 3);
    assert.match(
      state.previousError,
      /next.jobs: all watched results were already delivered/,
    );
    assert.ok(
      request.messages.some((message) =>
        message.content.includes("original request"),
      ),
    );
    return { ...wait, messages: [{ text: "repaired answer" }] };
  });
  try {
    h.server.addMessage("@helper original request");
    await h.agent.requestSync();
    await until(() => h.server.state.sends.length === 1);
    assert.equal(h.requests.length, 3);
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["repaired answer"],
    );
    assert.equal(Object.keys(h.store.state.sessions["conv-1"].jobs).length, 1);
    await h.restart();
    assert.equal(h.requests.length, 3);
  } finally {
    await h.close();
  }
});

test("job continuation retains every admitted input and the configured prompt", async () => {
  let release: ((value: string) => void) | undefined;
  const h = await fixture(async (request, call) => {
    assert.ok(
      request.messages.some((message) =>
        message.content.includes("Always speak concisely."),
      ),
    );
    if (call === 1) {
      request.executeTool = async () =>
        await new Promise<string>((resolve) => {
          release = resolve;
        });
      return {
        ...wait,
        actions: [
          { id: "work", tool: "execute_code", arguments: { code: "1" } },
        ],
        next: { kind: "on_jobs", jobs: ["work"], mode: "all" },
      };
    }
    const state = JSON.parse(
      request.messages
        .find((message) => message.content.startsWith("Tick state"))!
        .content.split("\n")
        .slice(1)
        .join("\n"),
    );
    if (call === 2)
      return {
        ...wait,
        next: { kind: "on_jobs", jobs: [state.jobs[0].id], mode: "all" },
      };
    assert.equal(call, 3);
    const context = request.messages
      .map((message) => message.content)
      .join("\n");
    assert.ok(context.includes("original work request"));
    assert.ok(context.includes("new requirement while working"));
    return { ...wait, messages: [{ text: "done" }] };
  });
  try {
    h.config.set("prompt", "Always speak concisely.");
    h.server.addMessage("@helper original work request");
    await h.agent.requestSync();
    await until(() => release !== undefined);
    h.server.addMessage("@helper new requirement while working");
    await h.agent.requestSync();
    release!("done");
    await until(() => h.server.state.sends.length === 1);
    assert.equal(h.requests.length, 3);
  } finally {
    release?.("cleanup");
    await h.close();
  }
});

test("reply ownership survives a delayed continuation and restart", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1
      ? {
          ...wait,
          messages: [{ text: "first answer" }],
          next: { kind: "after", delayMs: 100 },
        }
      : { ...wait, messages: [{ text: "another answer to the old question" }] },
  );
  try {
    h.server.addMessage("@helper one question");
    await h.agent.requestSync();
    await h.restart();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.deepEqual(
      h.server.state.sends.map((send) => send.text),
      ["first answer"],
    );
  } finally {
    await h.close();
  }
});

test("new input during a watched job gets its own answer and the old job cannot answer either input again", async () => {
  let release: ((value: string) => void) | undefined;
  const h = await fixture(async (request, call) => {
    if (call === 1) {
      request.executeTool = async () =>
        await new Promise<string>((resolve) => {
          release = resolve;
        });
      return {
        messages: [{ text: "first answer" }],
        actions: [
          { id: "work", tool: "execute_code", arguments: { code: "1" } },
        ],
        next: { kind: "on_jobs", jobs: ["work"], mode: "all" },
      };
    }
    if (call === 2) {
      const state = JSON.parse(
        request.messages
          .find((message) => message.content.startsWith("Tick state"))!
          .content.split("\n")
          .slice(1)
          .join("\n"),
      );
      return {
        ...wait,
        messages: [{ text: "second answer" }],
        next: { kind: "on_jobs", jobs: [state.jobs[0].id], mode: "all" },
      };
    }
    return { ...wait, messages: [{ text: "unwanted third answer" }] };
  });
  try {
    const first = h.server.addMessage("@helper first question");
    await h.agent.requestSync();
    await until(() => release !== undefined);
    const second = h.server.addMessage("@helper second question");
    await h.agent.requestSync();
    release!("done");
    await until(() => h.requests.length === 3);
    await h.agent.requestSync();
    assert.deepEqual(
      h.server.state.sends.map((send) => [send.text, send.replyTo]),
      [
        ["first answer", first.id],
        ["second answer", second.id],
      ],
    );
    await h.restart();
    assert.equal(h.server.state.sends.length, 2);
  } finally {
    release?.("cleanup");
    await h.close();
  }
});

test("a group continuation may answer a different input but cannot answer the first input twice", async () => {
  let first = "";
  let second = "";
  const h = await fixture(async (_request, call) =>
    call === 1
      ? {
          ...wait,
          messages: [{ text: "answer one", replyTo: first }],
          next: { kind: "continue" },
        }
      : {
          ...wait,
          messages: [
            { text: "duplicate one", replyTo: first },
            { text: "answer two", replyTo: second },
          ],
        },
  );
  try {
    h.config.set("autonomous", true);
    first = h.server.addMessage("first group question").id;
    second = h.server.addMessage("@helper second group question").id;
    await h.agent.requestSync();
    assert.deepEqual(
      h.server.state.sends.map((send) => [send.text, send.replyTo]),
      [
        ["answer one", first],
        ["answer two", second],
      ],
    );
    assert.deepEqual(h.requests[1].replyTargets, [second]);
  } finally {
    await h.close();
  }
});

test("wait lets a job finish silently without triggering inference", async () => {
  const h = await fixture(async () => ({
    ...wait,
    actions: [
      {
        id: "note",
        tool: "memory",
        arguments: { action: "set", key: "index", value: "quiet" },
      },
    ],
  }));
  try {
    h.server.addMessage("@helper remember quietly");
    await h.agent.requestSync();
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).some(
        (job) => job.status === "completed",
      ),
    );
    await h.agent.requestSync();
    assert.equal(h.requests.length, 1);
    assert.equal(h.server.state.sends.length, 0);
  } finally {
    await h.close();
  }
});

test("delayed continuation survives restart and a new input supersedes a stale timer", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1 ? { ...wait, next: { kind: "after", delayMs: 150 } } : wait,
  );
  try {
    h.server.addMessage("@helper later");
    await h.agent.requestSync();
    await h.restart();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.equal(h.requests.length, 2);
    h.server.addMessage("@helper another request");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 3);
  } finally {
    await h.close();
  }
});

test("self ticks consume an episode budget and local controls stay outside inference", async () => {
  const h = await fixture(async () => ({
    ...wait,
    next: { kind: "continue" },
  }));
  try {
    h.config.set("agent.maxTicks", 3);
    h.server.addMessage("@helper continue");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 3);
    assert.equal(h.store.state.sessions["conv-1"].phase, "failed");
    h.server.addMessage("@helper /ping");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 3);
    assert.equal(h.server.state.sends.at(-1)?.text, "Pong.");
  } finally {
    await h.close();
  }
});

test("new input during inference waits for the next tick without overlapping requests", async () => {
  let release: (() => void) | undefined;
  let active = 0;
  let maximum = 0;
  const h = await fixture(async (_request, call) => {
    active++;
    maximum = Math.max(maximum, active);
    if (call === 1)
      await new Promise<void>((resolve) => {
        release = resolve;
      });
    active--;
    return wait;
  });
  try {
    h.server.addMessage("@helper first");
    await h.agent.requestSync(true, false);
    await until(() => release !== undefined);
    h.server.addMessage("@helper second");
    await h.agent.requestSync(true, false);
    assert.equal(h.requests.length, 1);
    release!();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.equal(maximum, 1);
    assert.equal(h.requests[1].messages.at(-1)?.content, "@helper second");
  } finally {
    release?.();
    await h.close();
  }
});

test("a malformed decision starts no effects and unknown job watches are rejected", () => {
  assert.throws(() =>
    tickDecision(
      {
        ...wait,
        actions: [
          {
            id: "save",
            tool: "memory",
            arguments: { action: "set", value: 1 },
          },
          { id: "bad", tool: "unavailable", arguments: {} },
        ],
      },
      [MEMORY_TOOL],
    ),
  );
  assert.throws(() =>
    tickDecision({ ...wait, next: { kind: "after", delayMs: -1 } }, []),
  );
});

test("a slow room does not block a DM or local controls", async () => {
  let release: (() => void) | undefined;
  const h = await fixture(async (request) => {
    if (request.contextKey === "conv-1") {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return wait;
    }
    return { ...wait, messages: [{ text: "DM answered" }] };
  });
  try {
    const me = h.api.me.bind(h.api);
    const history = h.api.history.bind(h.api);
    const commands = h.api.serverCommands.bind(h.api);
    const send = h.api.send.bind(h.api);
    const messageContext = h.api.messageContext.bind(h.api);
    h.api.me = async () => {
      const snapshot = await me();
      return {
        ...snapshot,
        conversations: [
          ...snapshot.conversations,
          {
            ...snapshot.conversations[0],
            id: "direct-2",
            view: "@direct:alice",
          },
        ],
      };
    };
    h.api.history = (query, signal) =>
      history(
        {
          ...query,
          conversation_id:
            query.conversation_id === "direct-2"
              ? "conv-1"
              : query.conversation_id,
        },
        signal,
      );
    h.api.serverCommands = (id) => commands(id === "direct-2" ? "conv-1" : id);
    h.api.send = (request, key) =>
      send(
        {
          ...request,
          conversation_id:
            request.conversation_id === "direct-2"
              ? "conv-1"
              : request.conversation_id,
        },
        key,
      );
    h.api.messageContext = (id, message) =>
      messageContext(id === "direct-2" ? "conv-1" : id, message);
    await h.agent.sync();
    h.server.addMessage("@helper question");
    await h.agent.requestSync(true, false);
    await until(
      () =>
        release !== undefined &&
        h.server.state.sends.some((send) => send.text === "DM answered"),
    );
    assert.equal(h.requests.length, 2);
    h.server.addMessage("@helper /ping");
    await h.agent.requestSync(true, false);
    assert.ok(h.server.state.sends.some((send) => send.text === "Pong."));
    assert.equal(h.requests.length, 2);
    release!();
    await h.agent.requestSync();
  } finally {
    release?.();
    await h.close();
  }
});

test("new input replaces a job watch; the old completion cannot start an extra tick", async () => {
  let release: ((value: string) => void) | undefined;
  const h = await fixture(async (request, call) => {
    if (call === 1) {
      request.executeTool = () =>
        new Promise<string>((resolve) => {
          release = resolve;
        });
      return {
        ...wait,
        actions: [
          {
            id: "slow",
            tool: "memory",
            arguments: { action: "get", key: "index" },
          },
        ],
        next: { kind: "on_jobs", jobs: ["slow"], mode: "all" },
      };
    }
    return wait;
  });
  try {
    h.server.addMessage("@helper start");
    await h.agent.requestSync();
    await until(() => release !== undefined);
    h.server.addMessage("@helper leave that result for later");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 2);
    release!("completed result");
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).every(
        (job) => job.status === "completed",
      ),
    );
    await h.agent.requestSync();
    assert.equal(h.requests.length, 2);
    h.server.addMessage("@helper now use it");
    await h.agent.requestSync();
    assert.ok(
      h.requests[2].messages.some((message) =>
        message.content.includes("completed result"),
      ),
    );
  } finally {
    release?.("cleanup");
    await h.close();
  }
});

test("a superseded timer is ignored and no provider retries are hidden inside a tick", async (t) => {
  const h = await fixture(async (_request, call) =>
    call === 1 ? { ...wait, next: { kind: "after", delayMs: 75 } } : wait,
  );
  try {
    h.server.addMessage("@helper later");
    await h.agent.requestSync();
    h.server.addMessage("@helper now");
    await h.agent.requestSync();
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(h.requests.length, 2);
  } finally {
    await h.close();
  }
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    return new Response("unavailable", { status: 503 });
  });
  const model = new ModelClient({
    endpoint: "https://model.invalid",
    token: "",
    model: "model",
    timeoutMs: 1000,
    stream: false,
    log: () => {},
  });
  await assert.rejects(model.tick({ messages: [] }), /503/);
  assert.equal(calls, 1);
  t.mock.restoreAll();
});

test("retraction interrupts a watched job and suppresses late publication", async () => {
  let release: ((value: string) => void) | undefined;
  const h = await fixture(async (request, call) => {
    if (call !== 1) throw new Error("Retracted work must not tick again");
    request.executeTool = () =>
      new Promise<string>((resolve) => {
        release = resolve;
      });
    return {
      ...wait,
      actions: [
        {
          id: "slow",
          tool: "memory",
          arguments: { action: "get", key: "index" },
        },
      ],
      next: { kind: "on_jobs", jobs: ["slow"], mode: "all" },
    };
  });
  try {
    const input = h.server.addMessage("@helper start");
    await h.agent.requestSync();
    await until(() => release !== undefined);
    h.server.retractMessage(input.id);
    await h.agent.requestSync();
    release!("late result");
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).every(
        (job) => job.status === "interrupted",
      ),
    );
    assert.equal(h.requests.length, 1);
    assert.equal(h.server.state.sends.length, 0);
  } finally {
    release?.("cleanup");
    await h.close();
  }
});

test("invalid reply plans commit no jobs and repair on a separate tick", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1
      ? {
          messages: [{ text: "invalid", replyTo: "unrelated-message" }],
          actions: [
            {
              id: "write",
              tool: "memory",
              arguments: {
                action: "set",
                key: "index",
                value: "must not be written",
              },
            },
          ],
          next: { kind: "wait" },
        }
      : wait,
  );
  try {
    h.server.addMessage("@helper work");
    await h.agent.requestSync();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.equal(Object.keys(h.store.state.sessions["conv-1"].jobs).length, 0);
    assert.equal(h.server.state.sends.length, 0);
    assert.ok(
      h.requests[1].messages.some((message) =>
        message.content.includes("Reply target must be an admitted input"),
      ),
    );
    const repair = h.requests[1].messages.findIndex(
      (message) =>
        message.role === "system" &&
        message.content.includes("previous decision was rejected"),
    );
    assert.ok(
      repair >
        h.requests[1].messages.findIndex((message) =>
          message.content.startsWith("Your persistent workspace"),
        ),
    );
    assert.ok(
      h.logs.some(
        (line) =>
          line.includes("tick retry scheduled") &&
          line.includes("retry=1/2") &&
          line.includes("Reply target must be an admitted input"),
      ),
    );
  } finally {
    await h.close();
  }
});

test("native effects have distinct keys across ticks and recover known receipts after restart", async () => {
  let lost = false;
  const h = await fixture(async (_request, call) =>
    call <= 2
      ? {
          ...wait,
          actions: [
            {
              id: "command",
              tool: "run_server_command",
              arguments: { command: "/whoami" },
            },
          ],
          next: { kind: "on_jobs", jobs: ["command"], mode: "all" },
        }
      : wait,
  );
  try {
    const execute = h.api.executeCommand.bind(h.api);
    h.api.executeCommand = async (...args) => {
      const result = await execute(...args);
      if (!lost) {
        lost = true;
        throw new Error("lost committed response");
      }
      return result;
    };
    h.server.addMessage("@helper identity twice");
    await h.agent.requestSync();
    await until(() => h.requests.length === 3);
    await h.agent.requestSync();
    assert.equal(h.server.state.commands.length, 2);
    assert.equal(h.server.commandReceipts.size, 2);
    await h.restart();
    assert.equal(h.requests.length, 3);
    const actions =
      h.store.task("conv-1", h.store.state.sessions["conv-1"].root)?.actions ??
      [];
    assert.equal(actions.length, 2);
    assert.notEqual(actions[0].key, actions[1].key);
    assert.ok(actions.every((action) => action.status === "done"));
  } finally {
    await h.close();
  }
});

test("KV v1 migration retains accepted input, uncertain keys and pending output", async () => {
  const dir = mkdtempSync(join(tmpdir(), "simplebot-migrate-"));
  const path = join(dir, "agent.kv");
  let store = await Store.open(path);
  try {
    store.state.version = 1;
    store.state.account = "account";
    store.setCursor("conversation", 42);
    store.putTask({
      conversation: "conversation",
      input: "input",
      sequence: 42,
      promptHash: "hash",
      replyTo: null,
      status: "running",
      attempts: 1,
      slots: ["answer"],
      updated: Date.now(),
      actions: [
        {
          command: "/whoami",
          key: "original-command-key",
          status: "pending",
          created: Date.now(),
        },
      ],
    });
    store.putPart({
      slot: "conversation:input:answer",
      key: "original-output-key",
      request: { conversation_id: "conversation", text: "saved output" },
      created: Date.now(),
    });
    await store.close();
    store = await Store.open(path);
    assert.equal(store.state.version, 2);
    assert.equal(store.cursor("conversation"), 42);
    assert.equal(
      store.task("conversation", "input")?.actions?.[0].key,
      "original-command-key",
    );
    assert.equal(
      store.part("conversation:input:answer")?.request.text,
      "saved output",
    );
    assert.equal(
      store.part("conversation:input:answer")?.key,
      "original-output-key",
    );
    assert.equal((await Store.inspect(path)).version, 2);
  } finally {
    await store.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a full inbox pauses scanning and resumes when ticks free capacity", async () => {
  const h = await fixture(async () => wait);
  try {
    h.config.set("agent.inboxLimit", 10);
    let latest = 0;
    for (let index = 0; index < 14; index++)
      latest = h.server.addMessage(`@helper input ${index}`).sequence;
    await h.agent.requestSync();
    await until(() => h.requests.length === 14);
    await h.agent.requestSync();
    assert.equal(h.store.cursor("conv-1"), latest);
    assert.ok(h.logs.some((line) => line.includes("inbox_full")));
    assert.ok(h.store.tasks().every((task) => task.status === "answered"));
  } finally {
    await h.close();
  }
});

test("token usage is charged across self continuations without resetting the episode", async () => {
  const h = await fixture(async (request) => {
    request.onCall?.({
      success: true,
      durationMs: 1,
      usage: { prompt: 500, completion: 600 },
    });
    return { ...wait, next: { kind: "continue" } };
  });
  try {
    h.config.set("agent.maxEpisodeTokens", 1024);
    h.server.addMessage("@helper budget");
    await h.agent.requestSync();
    assert.equal(h.requests.length, 1);
    assert.equal(h.store.state.sessions["conv-1"].tokens, 1100);
    assert.equal(h.store.state.sessions["conv-1"].phase, "failed");
  } finally {
    await h.close();
  }
});

test("large job results are excerpted and can be paged within their conversation", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1) {
      request.executeTool = async () => "result-".repeat(2000);
      return {
        ...wait,
        actions: [
          {
            id: "large",
            tool: "memory",
            arguments: { action: "get", key: "index" },
          },
        ],
        next: { kind: "on_jobs", jobs: ["large"], mode: "all" },
      };
    }
    const stateMessage = request.messages.find((message) =>
      message.content.startsWith("Tick state (untrusted data"),
    )!;
    const state = JSON.parse(
      stateMessage.content.slice(stateMessage.content.indexOf("\n") + 1),
    ) as {
      jobs: {
        id: string;
        tool: string;
        result?: string;
        resultTruncated: boolean;
        observed: boolean;
      }[];
    };
    if (call === 2) {
      const result = state.jobs.find((job) => job.tool === "memory")!;
      assert.equal(result.result?.length, 4000);
      assert.equal(result.resultTruncated, true);
      assert.match(
        readJob(undefined, JSON.stringify({ job: result.id })),
        /Unknown job/,
      );
      return {
        ...wait,
        actions: [
          {
            id: "page",
            tool: "read_job",
            arguments: { job: result.id, offset: 5000, limit: 1000 },
          },
        ],
        next: { kind: "on_jobs", jobs: ["page"], mode: "all" },
      };
    }
    const page = JSON.parse(
      state.jobs.find((job) => job.tool === "read_job")!.result!,
    ) as { text: string; offset: number; nextOffset: number };
    assert.equal(page.text.length, 1000);
    assert.equal(page.offset, 5000);
    assert.equal(page.nextOffset, 6000);
    return wait;
  });
  try {
    h.server.addMessage("@helper large result");
    await h.agent.requestSync();
    await until(() => h.requests.length === 3);
    await h.agent.requestSync();
    assert.equal(h.store.state.sessions["conv-1"].phase, "idle");
    assert.ok(
      Object.values(h.store.state.sessions["conv-1"].jobs).some(
        (job) => job.result?.length === 14000,
      ),
    );
  } finally {
    await h.close();
  }
});

test("a committed job still dispatches if checkpoint completion crosses the inference deadline", async () => {
  const h = await fixture(async (_request, call) =>
    call === 1
      ? {
          ...wait,
          actions: [
            {
              id: "save",
              tool: "memory",
              arguments: {
                action: "set",
                key: "index",
                value: "saved after commit",
              },
            },
          ],
          next: { kind: "on_jobs", jobs: ["save"], mode: "all" },
        }
      : wait,
  );
  try {
    h.config.set("model.timeoutMs", 1000);
    const save = h.store.save.bind(h.store);
    let delayed = false;
    h.store.save = async () => {
      await save();
      if (!delayed && h.store.state.sessions["conv-1"]?.trace.length === 1) {
        delayed = true;
        await new Promise((resolve) => setTimeout(resolve, 1100));
      }
    };
    h.server.addMessage("@helper save");
    await h.agent.requestSync();
    await until(() => h.requests.length === 2);
    await h.agent.requestSync();
    assert.equal(
      Object.values(h.store.state.sessions["conv-1"].jobs)[0].status,
      "completed",
    );
  } finally {
    await h.close();
  }
});

test("shell wait expiry returns control while a watched command outlives the model timeout", async () => {
  const h = await fixture(async (request, call) => {
    assert.ok(
      request.tools?.some((tool) => tool.function.name === "run_shell"),
    );
    if (call === 1)
      return {
        ...wait,
        actions: [
          {
            id: "shell",
            tool: "run_shell",
            arguments: {
              command: "sleep 1.1; printf shell-result",
              waitMs: 10,
              timeoutMs: 3000,
            },
          },
        ],
        next: { kind: "on_jobs", jobs: ["shell"], mode: "all" },
      };
    const state = request.messages.find((message) =>
      message.content.startsWith("Tick state (untrusted data"),
    )!;
    assert.ok(state.content.includes("shell-result"));
    return { ...wait, messages: [{ text: "shell finished" }] };
  }, true);
  try {
    h.config.set("model.timeoutMs", 1000);
    h.server.addMessage("@helper shell command");
    await h.agent.requestSync();
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).some(
        (job) => job.waitExpired,
      ),
    );
    assert.equal(h.requests.length, 1);
    h.server.addMessage("@helper /ping");
    await h.agent.requestSync();
    assert.ok(h.server.state.sends.some((send) => send.text === "Pong."));
    await until(() =>
      h.server.state.sends.some((send) => send.text === "shell finished"),
    );
    const job = Object.values(h.store.state.sessions["conv-1"].jobs)[0];
    assert.equal(JSON.parse(job.result!).ok, true);
  } finally {
    await h.close();
  }
});

test("disabled shell requests are rejected even when bypassing the model catalogue", async () => {
  let captured: CompletionRequest | undefined;
  const h = await fixture(async (request) => {
    captured = request;
    return wait;
  });
  try {
    h.server.addMessage("@helper hello");
    await h.agent.requestSync();
    assert.ok(
      !captured?.tools?.some((tool) => tool.function.name === "run_shell"),
    );
    await assert.rejects(
      captured!.executeTool!("run_shell", '{"command":"printf bypass"}'),
      /--with-shell/,
    );
    h.config.set("prompt", "new personality");
    h.config.reload();
    assert.equal(h.agent.shell.enabled, false);
  } finally {
    await h.close();
  }
});

test("restart without --with-shell interrupts a shell job without replaying its command", async () => {
  const h = await fixture(async (request, call) => {
    if (call === 1)
      return {
        ...wait,
        actions: [
          {
            id: "shell",
            tool: "run_shell",
            arguments: {
              command:
                "printf started > started.txt; sleep 30; printf late > late.txt",
              waitMs: 0,
            },
          },
        ],
        next: { kind: "on_jobs", jobs: ["shell"], mode: "all" },
      };
    assert.ok(
      !request.tools?.some((tool) => tool.function.name === "run_shell"),
    );
    assert.ok(
      request.messages.some((message) =>
        message.content.includes('"status":"interrupted"'),
      ),
    );
    return wait;
  }, true);
  try {
    h.server.addMessage("@helper long shell command");
    await h.agent.requestSync();
    await until(() =>
      Object.values(h.store.state.sessions["conv-1"].jobs).some(
        (job) => job.waitExpired,
      ),
    );
    await h.restart(false);
    assert.equal(h.requests.length, 2);
    assert.equal(
      Object.values(h.store.state.sessions["conv-1"].jobs)[0].status,
      "interrupted",
    );
    assert.equal(h.agent.shell.enabled, false);
  } finally {
    await h.close();
  }
});

for (const stream of [false, true])
  for (const reasoningEffort of [undefined, "none"])
    test(`production adapter makes one compatible provider call per tick (stream=${stream}, reasoning=${reasoningEffort ?? "default"})`, async (t) => {
      let calls = 0;
      const decision = JSON.stringify(wait);
      t.mock.method(
        globalThis,
        "fetch",
        async (_url: unknown, init: RequestInit) => {
          calls++;
          const body = JSON.parse(String(init.body));
          // One envelope: actions are batched into finish_tick, so a tick never
          // spends a separate provider call per tool.
          assert.equal(body.tools.length, 1);
          // Thinking models can reject a forced named tool even with one tool.
          // Leave selection to the provider; tick() validates the decision.
          assert.equal(body.tool_choice, undefined);
          assert.equal(body.parallel_tool_calls, undefined);
          assert.equal(body.tools[0].function.name, "finish_tick");
          assert.deepEqual(
            body.tools[0].function.parameters.properties.actions.items.anyOf.map(
              (option: { properties: { tool: { enum: string[] } } }) =>
                option.properties.tool.enum[0],
            ),
            ["memory"],
          );
          assert.equal(body.reasoning_effort, reasoningEffort);
          const tool = {
            id: "tick-1",
            type: "function",
            function: { name: "finish_tick", arguments: decision },
          };
          if (!stream)
            return new Response(
              JSON.stringify({
                choices: [
                  {
                    message: { content: null, tool_calls: [tool] },
                    finish_reason: "tool_calls",
                  },
                ],
              }),
            );
          return new Response(
            `data: ${JSON.stringify({ choices: [{ delta: { tool_calls: [{ index: 0, ...tool }] }, finish_reason: null }] })}\n\ndata: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "tool_calls" }] })}\n\ndata: [DONE]\n\n`,
            { headers: { "Content-Type": "text/event-stream" } },
          );
        },
      );
      const model = new ModelClient({
        endpoint: "https://model.invalid",
        token: "",
        model: "model",
        timeoutMs: 1000,
        stream,
        log: () => {},
      });
      assert.deepEqual(
        await model.tick({
          messages: [{ role: "user", content: "hello" }],
          tools: [MEMORY_TOOL],
          reasoningEffort,
        }),
        wait,
      );
      assert.equal(calls, 1);
      t.mock.restoreAll();
    });

test("streamed tool deltas tolerate repeated ids/names and an omitted index", async (t) => {
  let attempts = 0;
  const fragments = [
    '{"messages":',
    '[],"actions":[],"next":{"kind":"wait"}',
    "}",
  ];
  t.mock.method(globalThis, "fetch", async () => {
    attempts++;
    // A compatible gateway that repeats id/name on every chunk and only sends
    // the index on the first one must not corrupt the call.
    const events = fragments.map((arguments_, index) => ({
      choices: [
        {
          delta: {
            tool_calls: [
              {
                ...(index === 0 ? { index: 0 } : {}),
                id: "tick-1",
                type: "function",
                function: { name: "finish_tick", arguments: arguments_ },
              },
            ],
          },
          finish_reason: index === fragments.length - 1 ? "tool_calls" : null,
        },
      ],
    }));
    return new Response(
      `${events.map((event) => `data: ${JSON.stringify(event)}`).join("\n\n")}\n\ndata: [DONE]\n\n`,
      { headers: { "Content-Type": "text/event-stream" } },
    );
  });
  const model = new ModelClient({
    endpoint: "https://model.invalid",
    token: "",
    model: "model",
    timeoutMs: 1000,
    stream: true,
    log: () => {},
  });
  assert.deepEqual(
    await model.tick({
      messages: [{ role: "user", content: "hello" }],
      tools: [MEMORY_TOOL],
    }),
    wait,
  );
  assert.equal(attempts, 1);
  t.mock.restoreAll();
});

for (const stream of [false, true])
  test(`automatic tool selection rejects ambiguous or unexpected function calls (stream=${stream})`, async (t) => {
    const validCall = {
      id: "tick-1",
      type: "function",
      function: { name: "finish_tick", arguments: JSON.stringify(wait) },
    };
    for (const calls of [
      [validCall, { ...validCall, id: "tick-2" }],
      [
        {
          ...validCall,
          function: { ...validCall.function, name: "unavailable" },
        },
      ],
    ]) {
      let attempts = 0;
      t.mock.method(globalThis, "fetch", async () => {
        attempts++;
        return stream
          ? new Response(
              `data: ${JSON.stringify({ choices: [{ delta: { content: "uncommitted text", tool_calls: calls.map((call, index) => ({ index, ...call })) }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`,
            )
          : Response.json({
              choices: [
                {
                  message: { content: "uncommitted text", tool_calls: calls },
                  finish_reason: "stop",
                },
              ],
            });
      });
      const model = new ModelClient({
        endpoint: "https://model.invalid",
        token: "",
        model: "model",
        timeoutMs: 1000,
        stream,
        log: () => {},
      });
      await assert.rejects(
        model.tick({
          messages: [{ role: "user", content: "hello" }],
          tools: [MEMORY_TOOL],
          executeTool: async () => {
            throw new Error("Effects must not run");
          },
        }),
        /Expected at most one finish_tick decision/,
      );
      assert.equal(attempts, 1);
      t.mock.restoreAll();
    }
  });

for (const stream of [false, true])
  test(`ordinary final text and silence end a tick without requesting a second answer (stream=${stream})`, async (t) => {
    let calls = 0;
    for (const text of ["ordinary final answer", ""]) {
      t.mock.method(
        globalThis,
        "fetch",
        async (_url: unknown, init: RequestInit) => {
          calls++;
          const body = JSON.parse(String(init.body));
          assert.equal(
            body.tools[0].function.parameters.properties.messages.items
              .properties.replyTo.enum,
            undefined,
          );
          return stream
            ? new Response(
                `data: ${JSON.stringify({ choices: [{ delta: { content: text }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`,
              )
            : Response.json({
                choices: [
                  { message: { content: text }, finish_reason: "stop" },
                ],
              });
        },
      );
      const model = new ModelClient({
        endpoint: "https://model.invalid",
        token: "",
        model: "model",
        timeoutMs: 1000,
        stream,
        log: () => {},
      });
      const before = calls;
      assert.deepEqual(
        await model.tick({
          messages: [{ role: "user", content: "question" }],
          replyTargets: ["input-id"],
        }),
        { ...wait, messages: text ? [{ text }] : [] },
      );
      assert.equal(calls - before, 1);
      t.mock.restoreAll();
    }
  });
