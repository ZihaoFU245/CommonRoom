import assert from "node:assert/strict";
import { test } from "node:test";
import { Metrics } from "../src/metrics.ts";
import { ModelClient, type ModelCall } from "../src/model.ts";
import {
  compactConversation,
  estimateTokens,
  ProviderTokens,
} from "../src/tokens.ts";

for (const providerFailure of [false, true])
  test(`tick telemetry counts HTTP 422 and plan rejection exactly once (upstream=${providerFailure})`, async (t) => {
    const calls: ModelCall[] = [];
    t.mock.method(globalThis, "fetch", async () =>
      providerFailure
        ? Response.json(
            { error: { message: "synthetic rejection" } },
            { status: 422 },
          )
        : Response.json({
            choices: [
              {
                message: {
                  content: null,
                  tool_calls: [
                    {
                      id: "bad",
                      type: "function",
                      function: {
                        name: "finish_tick",
                        arguments: "invalid JSON",
                      },
                    },
                  ],
                },
                finish_reason: "tool_calls",
              },
            ],
          }),
    );
    const client = new ModelClient({
      endpoint: "https://model.invalid",
      model: "test",
      token: "",
      stream: false,
      timeoutMs: 1000,
      log: () => {},
    });
    await assert.rejects(
      client.tick({
        messages: [{ role: "user", content: "synthetic" }],
        onCall: (call) => calls.push(call),
      }),
    );
    assert.equal(calls.length, 1);
    assert.equal(calls[0].success, !providerFailure);
  });

test("metrics separate preparation, inference and reported first streamed deltas", () => {
  const metrics = new Metrics();
  metrics.prepared(10);
  metrics.prepared(30);
  metrics.record({ success: true, durationMs: 100, firstDeltaMs: 40 });
  metrics.record({ success: true, durationMs: 200 });
  assert.match(metrics.report(), /\| Context preparation \| 30 ms \| 20 ms/);
  assert.match(metrics.report(), /\| Model attempt \| 200 ms \| 150 ms/);
  assert.match(
    metrics.report(),
    /\| First streamed delta \| 40 ms \| 40 ms \| 40 ms \| 1 \|/,
  );
});

test("provider measurements anchor exact requests and only new suffixes need estimates", () => {
  const meter = new ProviderTokens();
  const prefix = {
    role: "system" as const,
    content: "stable instructions ".repeat(100),
  };
  const messages = [prefix, { role: "user" as const, content: "old question" }];
  assert.equal(meter.count(messages).source, "estimate");
  meter.observe(messages, undefined, undefined, 100);
  assert.deepEqual(meter.count(messages), { tokens: 100, source: "provider" });
  const appended = [
    ...messages,
    { role: "assistant" as const, content: "new answer 中文" },
  ];
  const next = meter.count(appended);
  assert.equal(next.source, "provider+estimate");
  assert.ok(next.tokens > 100);
  assert.ok(next.tokens < estimateTokens({ messages: appended }));
  const replaced = [prefix, { role: "user" as const, content: "new question" }];
  assert.equal(meter.count(replaced).source, "provider+estimate");
  // Removing a suffix cannot subtract an invented exact token count.
  assert.ok(meter.count([prefix]).tokens >= 100);
  assert.equal(meter.count(messages, [], "high").source, "estimate");
  assert.equal(
    meter.count([{ role: "user", content: "unrelated" }]).source,
    "estimate",
  );
  meter.observe(appended, undefined, undefined, 150);
  assert.deepEqual(meter.count(appended), { tokens: 150, source: "provider" });
  meter.reset();
  assert.equal(meter.count(appended).source, "estimate");
});

test("provider measurements are scoped by conversation and do not make extra calls", async (t) => {
  const calls: ModelCall[] = [];
  let requests = 0;
  t.mock.method(globalThis, "fetch", async () => {
    requests++;
    return Response.json({
      choices: [{ message: { content: "ok" }, finish_reason: "stop" }],
      usage: { prompt_tokens: 100, completion_tokens: 1 },
    });
  });
  const client = new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    timeoutMs: 1000,
    stream: false,
    log: () => {},
  });
  const messages = [
    { role: "system" as const, content: "fixed ".repeat(200) },
    { role: "user" as const, content: "hello" },
  ];
  const run = (
    contextKey: string,
    resetTokenBaseline = false,
    input = messages,
  ) =>
    client.complete({
      messages: input,
      contextKey,
      resetTokenBaseline,
      onCall: (call) => calls.push(call),
    });
  await run("room-a");
  await run("room-a");
  await run("room-a", false, [
    ...messages,
    { role: "user", content: "appended" },
  ]);
  await run("room-b");
  await run("room-a", true);
  assert.equal(requests, 5);
  assert.deepEqual(
    calls.map((call) => call.inputBudget?.source),
    ["estimate", "provider", "provider+estimate", "estimate", "estimate"],
  );
  assert.equal(calls[1]!.inputBudget!.tokens, 100);
  const metrics = new Metrics();
  metrics.record(calls[1]!);
  assert.match(
    metrics.report(),
    /Latest request input\/output \| 100 \/ 1 tokens/,
  );
  assert.match(
    metrics.report(),
    /Pre-request input budget \| 100 tokens \(provider\)/,
  );
});

test("actual provider usage triggers compaction even when the first estimate was below the threshold", async (t) => {
  let requests = 0;
  let compactions = 0;
  t.mock.method(globalThis, "fetch", async () => {
    requests++;
    return Response.json({
      choices: [{ message: { content: "ok" }, finish_reason: "stop" }],
      usage: {
        prompt_tokens: requests === 1 ? 3500 : 800,
        completion_tokens: 1,
      },
    });
  });
  const client = new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    timeoutMs: 1000,
    stream: false,
    log: () => {},
  });
  const history =
    "Conversation data [sequence,id,sender,text,offset,author_id]:\n" +
    JSON.stringify({
      messages: Array.from({ length: 10 }, (_, i) => [
        i,
        `id-${i}`,
        "alice",
        "x".repeat(500),
      ]),
    });
  const messages = [
    { role: "system" as const, content: "fixed" },
    { role: "user" as const, content: history },
    { role: "user" as const, content: "question" },
  ];
  const request = {
    messages,
    contextKey: "room",
    contextTokens: 4096,
    compactTokens: 3072,
    maxTokens: 128,
    compact: (input: typeof messages, target: number, ratio: number) => {
      compactions++;
      return compactConversation(input, undefined, target, ratio);
    },
  };
  await client.complete(request);
  assert.equal(compactions, 0);
  await client.complete(request);
  assert.equal(compactions, 1);
  assert.equal(requests, 2);
});

test("a low measured input count prevents premature compaction from a high byte estimate", async (t) => {
  let compactions = 0;
  const calls: ModelCall[] = [];
  t.mock.method(globalThis, "fetch", async () =>
    Response.json({
      choices: [{ message: { content: "ok" }, finish_reason: "stop" }],
      usage: { prompt_tokens: 1000, completion_tokens: 1 },
    }),
  );
  const client = new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    timeoutMs: 1000,
    stream: false,
    log: () => {},
  });
  const messages = [{ role: "user" as const, content: "x".repeat(12000) }];
  const base = {
    messages,
    contextKey: "room",
    maxTokens: 128,
    onCall: (call: ModelCall) => calls.push(call),
    compact: (input: typeof messages) => {
      compactions++;
      return input;
    },
  };
  await client.complete({ ...base, contextTokens: 16384, compactTokens: 8192 });
  await client.complete({ ...base, contextTokens: 4096, compactTokens: 3072 });
  assert.equal(compactions, 0);
  assert.deepEqual(calls[1]!.inputBudget, { tokens: 1000, source: "provider" });
});

test("an unchanged compact result keeps the exact measurement and reserves measured remaining output", async (t) => {
  const budgets: ModelCall[] = [];
  let requests = 0;
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init?: RequestInit) => {
      requests++;
      const body = JSON.parse(String(init?.body));
      if (requests === 2) assert.equal(body.max_tokens, 196);
      return Response.json({
        choices: [{ message: { content: "ok" }, finish_reason: "stop" }],
        usage: { prompt_tokens: 3900, completion_tokens: 1 },
      });
    },
  );
  const client = new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    timeoutMs: 1000,
    stream: false,
    log: () => {},
  });
  const request = {
    messages: [{ role: "user" as const, content: "fixed input" }],
    contextKey: "room",
    contextTokens: 4096,
    compactTokens: 3072,
    maxTokens: 256,
    compact: (messages: { role: "user"; content: string }[]) => messages,
    onCall: (call: ModelCall) => budgets.push(call),
  };
  await client.complete(request);
  await client.complete(request);
  assert.deepEqual(budgets[1]!.inputBudget, {
    tokens: 3900,
    source: "provider",
  });
});

for (const stream of [true, false])
  test(`model usage telemetry handles cached, zero and absent counters (stream=${stream})`, async (t) => {
    const calls: ModelCall[] = [];
    const metrics = new Metrics();
    let round = 0;
    const usages = [
      {
        prompt_tokens: 100,
        completion_tokens: 12,
        prompt_cache_hit_tokens: 60,
      },
      {
        prompt_tokens: 100,
        completion_tokens: 8,
        prompt_tokens_details: { cached_tokens: 0 },
      },
      undefined,
      {
        prompt_tokens: -1,
        completion_tokens: "secret",
        prompt_cache_hit_tokens: -2,
      },
    ];
    t.mock.method(
      globalThis,
      "fetch",
      async (_url: unknown, init?: RequestInit) => {
        const body = JSON.parse(String(init?.body));
        if (stream)
          assert.deepEqual(body.stream_options, { include_usage: true });
        const usage = usages[round++];
        return stream
          ? new Response(
              `data: ${JSON.stringify({ choices: [{ delta: { content: "answer" }, finish_reason: "stop" }] })}\n\ndata: ${JSON.stringify({ choices: [], usage })}\n\ndata: [DONE]\n\n`,
            )
          : Response.json({
              choices: [
                { message: { content: "answer" }, finish_reason: "stop" },
              ],
              usage,
            });
      },
    );
    const client = new ModelClient({
      endpoint: "https://model.invalid/",
      token: "",
      model: "test",
      timeoutMs: 1000,
      stream,
      log: () => {},
    });
    for (let i = 0; i < usages.length; i++)
      await client.complete({
        messages: [{ role: "user", content: "synthetic prompt" }],
        onCall: (call) => {
          calls.push(call);
          metrics.record(call);
        },
      });
    assert.equal(calls.length, 4);
    for (const call of calls)
      if (stream) {
        assert.equal(typeof call.firstDeltaMs, "number");
        assert.ok((call.firstDeltaMs ?? -1) >= 0);
        assert.ok((call.firstDeltaMs ?? Infinity) <= call.durationMs);
      } else assert.equal(call.firstDeltaMs, undefined);
    assert.equal(calls[0]!.usage?.cached, 60);
    assert.equal(calls[1]!.usage?.cached, 0);
    assert.equal(calls[2]!.usage, undefined);
    assert.equal(calls[3]!.usage?.prompt, undefined);
    assert.match(
      metrics.report(),
      /Cached input tokens \| 60 \(2\/4 attempts reported\)/,
    );
    assert.match(metrics.report(), /30\.0%/);
  });

test("unsupported streaming usage falls back once and counts the failed attempt", async (t) => {
  const calls: ModelCall[] = [];
  const bodies: Record<string, unknown>[] = [];
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init?: RequestInit) => {
      bodies.push(JSON.parse(String(init?.body)));
      if (bodies.length === 1)
        return Response.json(
          { error: "unsupported stream_options" },
          { status: 400 },
        );
      return new Response(
        'data: {"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n',
      );
    },
  );
  const client = new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    timeoutMs: 1000,
    stream: true,
    log: () => {},
  });
  for (let i = 0; i < 2; i++)
    await client.complete({
      messages: [{ role: "user", content: "hello" }],
      onCall: (call) => calls.push(call),
    });
  assert.equal(calls.length, 3);
  assert.equal(calls[0]!.success, false);
  assert.equal(bodies[1]!.stream_options, undefined);
  assert.equal(bodies[2]!.stream_options, undefined);
});

test("model compacts before a request and reserves output inside the total token limit", async (t) => {
  let compacted = 0;
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body));
      assert.ok(
        estimateTokens({ messages: body.messages, tools: body.tools }) +
          body.max_tokens <=
          4096,
      );
      assert.ok(
        body.messages.some(
          (message: { content: string }) =>
            message.content === "current question",
        ),
      );
      return Response.json({
        choices: [{ message: { content: "ok" }, finish_reason: "stop" }],
      });
    },
  );
  const client = new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    timeoutMs: 1000,
    stream: false,
    log: () => {},
  });
  const history =
    "Conversation data [sequence,id,sender,text,offset,author_id]:\n" +
    JSON.stringify({
      messages: Array.from({ length: 50 }, (_, i) => [
        i,
        `id-${i}`,
        "alice",
        "x".repeat(500),
      ]),
    });
  await client.complete({
    messages: [
      { role: "system", content: "fixed instructions" },
      { role: "user", content: history },
      { role: "user", content: "current question" },
    ],
    contextTokens: 4096,
    compactTokens: 3072,
    maxTokens: 2048,
    compact: (messages, target, ratio) => {
      compacted++;
      return compactConversation(messages, undefined, target, ratio);
    },
  });
  assert.equal(compacted, 1);
  await assert.rejects(
    client.complete({
      messages: [{ role: "user", content: "x".repeat(20000) }],
      contextTokens: 4096,
    }),
    /context budget exhausted/,
  );
});

test("compaction during a tool loop preserves IDs and never repeats the action", async (t) => {
  let calls = 0;
  let effects = 0;
  let compactions = 0;
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init?: RequestInit) => {
      const body = JSON.parse(String(init?.body));
      assert.ok(
        estimateTokens({ messages: body.messages, tools: body.tools }) +
          body.max_tokens <=
          4096,
      );
      calls++;
      if (calls === 1)
        return Response.json({
          choices: [
            {
              message: {
                content: "",
                tool_calls: [
                  {
                    id: "committed",
                    type: "function",
                    function: { name: "action", arguments: "{}" },
                  },
                ],
              },
              finish_reason: "tool_calls",
            },
          ],
        });
      assert.equal(body.messages.at(-1).role, "tool");
      assert.equal(body.messages.at(-1).tool_call_id, "committed");
      assert.match(body.messages.at(-1).content, /Result truncated/);
      assert.equal(body.messages.at(-2).tool_calls[0].id, "committed");
      return Response.json({
        choices: [{ message: { content: "done" }, finish_reason: "stop" }],
      });
    },
  );
  const client = new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    timeoutMs: 1000,
    stream: false,
    log: () => {},
  });
  const result = await client.complete({
    messages: [{ role: "user", content: "do the action" }],
    tools: [
      {
        type: "function",
        function: {
          name: "action",
          description: "action",
          parameters: { type: "object" },
        },
      },
    ],
    contextTokens: 4096,
    compactTokens: 3072,
    maxTokens: 1024,
    compact: (messages, target, ratio) => {
      compactions++;
      return compactConversation(messages, undefined, target, ratio);
    },
    executeTool: async () => {
      effects++;
      return "committed action result " + "x".repeat(16000);
    },
  });
  assert.equal(result, "done");
  assert.equal(calls, 2);
  assert.equal(effects, 1);
  assert.equal(compactions, 1);
});
