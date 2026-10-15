import assert from "node:assert/strict";
import { test } from "node:test";
import { BotApi, BotApiError, idempotencyKey } from "../src/api.ts";
import { ModelClient, completionsUrl } from "../src/model.ts";
import { BotSocket, type SocketEvents } from "../src/socket.ts";
import { parseServerFrame } from "../src/wire.ts";
import { MockServer } from "./mock-server.ts";

const api = () =>
  new BotApi({ baseUrl: "http://localhost/commonroom/", token: "bot_test" });
const model = (stream = true) =>
  new ModelClient({
    endpoint: "https://model.invalid/",
    token: "",
    model: "test",
    stream,
    timeoutMs: 3000,
    log: () => {},
  });
const completion = {
  messages: [{ role: "user" as const, content: "synthetic prompt" }],
};

for (const stream of [true, false])
  test(`native function calls round-trip with tool results (stream=${stream})`, async (t) => {
    const bodies: Record<string, unknown>[] = [];
    let executions = 0;
    t.mock.method(
      globalThis,
      "fetch",
      async (_url: string | URL | Request, init?: RequestInit) => {
        const body = JSON.parse(String(init?.body)) as Record<string, unknown>;
        bodies.push(body);
        if (bodies.length === 1) {
          const call = {
            id: "call-1",
            type: "function",
            function: {
              name: "run_server_command",
              arguments: '{"command":"/whoami"}',
            },
          };
          return stream
            ? new Response(
                [
                  'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"run_server_command","arguments":"{\\\"command\\\":"}}]}}]}\n\n',
                  'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\\\"/whoami\\\"}"}}]},"finish_reason":"tool_calls"}]}\n\n',
                  "data: [DONE]\n\n",
                ].join(""),
              )
            : Response.json({
                choices: [
                  {
                    message: { content: null, tool_calls: [call] },
                    finish_reason: "tool_calls",
                  },
                ],
              });
        }
        return stream
          ? new Response(
              'data: {"choices":[{"delta":{"content":"I am helper"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n',
            )
          : Response.json({
              choices: [
                { message: { content: "I am helper" }, finish_reason: "stop" },
              ],
            });
      },
    );
    const answer = await model(stream).complete({
      ...completion,
      tools: [
        {
          type: "function",
          function: {
            name: "run_server_command",
            description: "Run a server command",
            parameters: { type: "object" },
          },
        },
      ],
      executeTool: async (name, args) => {
        executions++;
        assert.equal(name, "run_server_command");
        assert.deepEqual(JSON.parse(args), { command: "/whoami" });
        return '{"result":"helper admin"}';
      },
    });
    assert.equal(answer, "I am helper");
    assert.equal(executions, 1);
    assert.equal(bodies.length, 2);
    const messages = bodies[1]?.messages as {
      role: string;
      tool_call_id?: string;
      content: string;
    }[];
    assert.equal(messages.at(-1)?.role, "tool");
    assert.equal(messages.at(-1)?.tool_call_id, "call-1");
  });

test("generic server commands retry with the same persisted key", async (t) => {
  let attempts = 0;
  const keys: unknown[] = [];
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: string | URL | Request, init?: RequestInit) => {
      attempts++;
      keys.push(new Headers(init?.headers).get("idempotency-key"));
      if (attempts === 1) throw new Error("lost connection");
      return new Response(
        JSON.stringify({
          result: "created",
          replayed: true,
          expires: 2000000000,
        }),
      );
    },
  );
  const result = await api().executeCommand(
    "room-id",
    "/new room demo",
    "persisted-key",
  );
  assert.equal(result.replayed, true);
  assert.equal(attempts, 2);
  assert.deepEqual(keys, ["persisted-key", "persisted-key"]);
});

test("creation is never retried after an ambiguous transport failure", async (t) => {
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    throw new Error("connection lost");
  });
  await assert.rejects(
    api().createConversation({ kind: "room", name: "demo" }),
    BotApiError,
  );
  assert.equal(calls, 1);
});

test("pinned revision conflicts return immediately for a fresh rescan", async (t) => {
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    return Response.json(
      { error: { code: "conflict", message: "changed", retryable: true } },
      { status: 409 },
    );
  });
  await assert.rejects(
    api().history({ conversation_id: "room", content_revision: 1 }),
    BotApiError,
  );
  assert.equal(calls, 1);
});

test("desired-state reactions retry transient failures with the same body", async (t) => {
  const requests: RequestInit[] = [];
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init: RequestInit) => {
      requests.push(init);
      return requests.length === 1
        ? Response.json(
            {
              error: { code: "unavailable", message: "busy", retryable: true },
            },
            { status: 503 },
          )
        : Response.json({ ok: true }, { headers: { "Bot-Api-Version": "1" } });
    },
  );
  await api().react("room", "message", "👍", true);
  assert.equal(requests.length, 2);
  assert.equal(requests[0]?.body, requests[1]?.body);
});

test("malformed HTTP and socket responses are rejected at the boundary", async (t) => {
  t.mock.method(globalThis, "fetch", async () =>
    Response.json({ kind: "snapshot", account: null, conversations: [] }),
  );
  await assert.rejects(api().me(), /Invalid Bot API me response/);
  for (const frame of [
    null,
    { kind: "error", id: null, error: null },
    { kind: "snapshot", account: null, conversations: [] },
    { kind: "presence", online: [7] },
    {
      kind: "config_request",
      id: "id",
      operation: "fetch",
      key: null,
      value: null,
    },
  ]) {
    assert.equal(parseServerFrame(frame), null);
  }
});

test("idempotency keys remain valid and distinct for long opaque IDs", () => {
  const a = idempotencyKey("room:" + "x".repeat(120), "message", "answer:1");
  assert.match(a, /^[A-Za-z0-9._:-]{1,128}$/);
  assert.equal(
    a,
    idempotencyKey("room:" + "x".repeat(120), "message", "answer:1"),
  );
  assert.notEqual(
    a,
    idempotencyKey("room:" + "x".repeat(120), "message", "answer:2"),
  );
});

test("model base URLs and explicit completion paths resolve correctly", () => {
  for (const base of ["https://host/", "https://host/v1", "https://host/v1/"])
    assert.equal(completionsUrl(base), "https://host/v1/chat/completions");
  assert.equal(
    completionsUrl("https://host/prefix/v1/chat/completions"),
    "https://host/prefix/v1/chat/completions",
  );
});

test("streaming handles split UTF-8, CRLF, and the final event without a newline", async (t) => {
  const bytes = new TextEncoder().encode(
    'data: {"choices":[{"delta":{"content":"你好 **world**"}}]}\r\n\r\ndata: [DONE]',
  );
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init: RequestInit) => {
      assert.equal(
        (init.headers as Record<string, string>).Authorization,
        undefined,
        "keyless model omits auth",
      );
      return new Response(
        new ReadableStream({
          start(controller) {
            for (const byte of bytes)
              controller.enqueue(new Uint8Array([byte]));
            controller.close();
          },
        }),
      );
    },
  );
  assert.equal(await model().complete(completion), "你好 **world**");
});

test("streaming error events fail instead of publishing a partial answer", async (t) => {
  t.mock.method(
    globalThis,
    "fetch",
    async () =>
      new Response(
        'data: {"choices":[{"delta":{"content":"partial"}}]}\n\ndata: {"error":{"message":"private provider detail"}}\n\n',
      ),
  );
  await assert.rejects(
    model().complete(completion),
    /model stream reported an error/,
  );
});

test("buffered model responses work without a key and errors do not echo response bodies", async (t) => {
  t.mock.method(globalThis, "fetch", async () =>
    Response.json({ choices: [{ message: { content: "**ready**" } }] }),
  );
  assert.equal(await model(false).complete(completion), "**ready**");
  t.mock.method(
    globalThis,
    "fetch",
    async () => new Response("secret provider body", { status: 401 }),
  );
  await assert.rejects(
    model(false).complete(completion),
    (error: unknown) =>
      error instanceof Error &&
      error.message === "model gateway returned HTTP 401",
  );
});

class FakeWebSocket {
  static latest: FakeWebSocket;
  readyState = 1;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  readonly sent: unknown[] = [];
  constructor() {
    FakeWebSocket.latest = this;
  }
  send(raw: string) {
    this.sent.push(JSON.parse(raw));
  }
  close() {
    this.readyState = 3;
  }
  receive(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
}

for (const supported of [true, false]) {
  test(`socket activity respects advertised support (${supported}) and credential scopes`, async (t) => {
    const original = globalThis.WebSocket;
    globalThis.WebSocket = FakeWebSocket as unknown as typeof WebSocket;
    t.after(() => {
      globalThis.WebSocket = original;
    });
    const events: SocketEvents = {
      snapshot() {},
      conversation() {},
      presence() {},
      activity() {},
      commandsRequest() {},
      socketError() {},
      status() {},
      lagged() {},
      sync() {},
      standby() {},
    };
    const socket = new BotSocket({
      url: "ws://localhost/",
      headers: {},
      events,
      supportsActivity: async () => supported,
      log: () => {},
    });
    t.after(() => socket.close());
    socket.open();
    const wire = FakeWebSocket.latest;
    wire.onopen?.();
    await Promise.resolve();
    const snapshot = new MockServer().snapshot();
    wire.receive(snapshot);
    socket.activity("conv-1", "working");
    socket.activity("conv-1", "idle");
    assert.equal(wire.sent.length, supported ? 3 : 1);
    snapshot.credential.operations = ["read", "send"];
    wire.receive(snapshot);
    socket.activity("conv-1", "working");
    assert.equal(wire.sent.length, supported ? 3 : 1);
    assert.deepEqual(wire.sent[0], { kind: "commands_ready" });
    assert.ok(
      !wire.sent.some((frame) =>
        JSON.stringify(frame).includes("config_ready"),
      ),
    );
    wire.receive({ kind: "error", id: null, error: null });
  });
}

test("incomplete model streams are retried instead of posting a partial answer", async (t) => {
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    return new Response(
      'data: {"choices":[{"delta":{"content":"partial"}}]}\n\n',
    );
  });
  await assert.rejects(model().complete(completion), /ended before completion/);
  assert.equal(calls, 3);
});

test("unconfigured models can be described safely", () => {
  const client = new ModelClient({
    endpoint: "",
    model: "",
    token: "",
    stream: true,
    timeoutMs: 1000,
    log: () => {},
  });
  assert.equal(client.describe, "not configured");
});

test("schema discovery distinguishes legacy and activity-capable servers", async (t) => {
  t.mock.method(globalThis, "fetch", async () =>
    Response.json({
      definitions: {
        ClientFrame: { oneOf: [{ $ref: "#/definitions/ConfigReady" }] },
      },
    }),
  );
  assert.equal(await api().supportsActivity(), false);
  t.mock.method(globalThis, "fetch", async () =>
    Response.json({
      definitions: {
        ClientFrame: { oneOf: [{ $ref: "#/definitions/ActivityRequest" }] },
      },
    }),
  );
  assert.equal(await api().supportsActivity(), true);
});

for (const stream of [true, false]) {
  test(`reasoning exhausting the output budget fails clearly without identical retries (stream=${stream})`, async (t) => {
    let calls = 0;
    t.mock.method(globalThis, "fetch", async () => {
      calls++;
      return stream
        ? new Response(
            'data: {"choices":[{"delta":{"reasoning_content":"synthetic reasoning"},"finish_reason":null}]}\n\ndata: {"choices":[{"delta":{},"finish_reason":"length"}]}\n\ndata: [DONE]\n\n',
          )
        : Response.json({
            choices: [
              {
                message: {
                  content: "",
                  reasoning_content: "synthetic reasoning",
                },
                finish_reason: "length",
              },
            ],
          });
    });
    await assert.rejects(
      model(stream).complete({ ...completion, maxTokens: 900 }),
      (error: unknown) => {
        assert.ok(error instanceof Error);
        assert.match(error.message, /output token limit \(max_tokens=900\)/);
        assert.ok(!error.message.includes("synthetic reasoning"));
        return true;
      },
    );
    assert.equal(calls, 1, "the same insufficient budget must not be retried");
  });
}

test("model requests forward the token budget and optional reasoning control", async (t) => {
  const bodies: Record<string, unknown>[] = [];
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init: RequestInit) => {
      bodies.push(JSON.parse(String(init.body)) as Record<string, unknown>);
      return Response.json({
        choices: [
          { message: { content: "**answer**" }, finish_reason: "stop" },
        ],
      });
    },
  );
  await model(false).complete({
    ...completion,
    maxTokens: 4096,
    reasoningEffort: "none",
  });
  assert.equal(bodies[0]?.max_tokens, 4096);
  assert.equal(bodies[0]?.reasoning_effort, "none");
  await model(false).complete(completion);
  assert.equal(Object.hasOwn(bodies[1]!, "reasoning_effort"), false);
});

test("unpinned history conflicts retry a fresh page while pinned rescans fail immediately", async (t) => {
  let calls = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    if (calls === 1)
      return new Response(
        JSON.stringify({
          error: {
            code: "conflict",
            message: "history changed",
            retryable: true,
          },
        }),
        { status: 409 },
      );
    return new Response(
      JSON.stringify({
        conversation_id: "room",
        messages: [],
        content_revision: 2,
        has_before: false,
        has_after: false,
      }),
    );
  });
  assert.equal(
    (await api().history({ conversation_id: "room", limit: 2 }))
      .content_revision,
    2,
  );
  assert.equal(calls, 2);
});

for (const stream of [false, true]) {
  test(`an explicit empty completion finishes silently (stream=${stream})`, async (t) => {
    t.mock.method(globalThis, "fetch", async () =>
      stream
        ? new Response(
            'data: {"choices":[{"delta":{},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n',
          )
        : Response.json({
            choices: [{ message: { content: "" }, finish_reason: "stop" }],
          }),
    );
    const client = new ModelClient({
      endpoint: "http://localhost/",
      model: "test",
      token: "",
      stream,
      timeoutMs: 1000,
      log: () => {},
    });
    assert.equal(await client.complete(completion), "");
  });
}

for (const stream of [false, true]) {
  test(`sixteen tool calls execute in one model round (stream=${stream})`, async (t) => {
    let rounds = 0,
      executed = 0;
    t.mock.method(globalThis, "fetch", async () => {
      rounds++;
      const calls = Array.from({ length: 16 }, (_, index) => ({
        id: `call-${index}`,
        type: "function",
        function: { name: "step", arguments: "{}" },
      }));
      return stream
        ? new Response(
            `data: ${JSON.stringify({ choices: [{ delta: rounds === 1 ? { tool_calls: calls.map((call, index) => ({ index, ...call })) } : { content: "done" }, finish_reason: rounds === 1 ? "tool_calls" : "stop" }] })}\n\ndata: [DONE]\n\n`,
          )
        : Response.json({
            choices: [
              {
                message:
                  rounds === 1
                    ? { content: null, tool_calls: calls }
                    : { content: "done" },
                finish_reason: rounds === 1 ? "tool_calls" : "stop",
              },
            ],
          });
    });
    assert.equal(
      await model(stream).complete({
        ...completion,
        maxToolCalls: 16,
        tools: [
          {
            type: "function",
            function: { name: "step", description: "step", parameters: {} },
          },
        ],
        executeTool: async () => {
          executed++;
          return "ok";
        },
      }),
      "done",
    );
    assert.equal(executed, 16);
  });
}

test("HTTP failure is handled before reading an upstream body that never ends", async (t) => {
  let calls = 0,
    cancelled = false;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    return calls === 1
      ? new Response(
          new ReadableStream({
            cancel() {
              cancelled = true;
            },
          }),
          { status: 503 },
        )
      : Response.json({
          choices: [
            { message: { content: "recovered" }, finish_reason: "stop" },
          ],
        });
  });
  assert.equal(await model(false).complete(completion), "recovered");
  assert.equal(calls, 2);
  assert.equal(cancelled, true);
});

test("overall model deadline releases a fetch implementation that ignores abort", async (t) => {
  t.mock.method(
    globalThis,
    "fetch",
    async () => new Promise<Response>(() => {}),
  );
  const client = new ModelClient({
    endpoint: "http://localhost/",
    model: "test",
    token: "",
    stream: false,
    timeoutMs: 50,
    log: () => {},
  });
  const alive = setTimeout(() => {}, 100);
  try {
    await assert.rejects(
      client.complete(completion),
      (error: unknown) =>
        error instanceof Error && error.name === "TimeoutError",
    );
  } finally {
    clearTimeout(alive);
  }
});

test("a finishing tool ends the turn without an extra model request or later effects", async (t) => {
  let calls = 0,
    finished = false,
    executed = 0;
  t.mock.method(globalThis, "fetch", async () => {
    calls++;
    return Response.json({
      choices: [
        {
          message: {
            content: null,
            tool_calls: ["finish", "late"].map((name) => ({
              id: name,
              type: "function",
              function: { name, arguments: "{}" },
            })),
          },
          finish_reason: "tool_calls",
        },
      ],
    });
  });
  const text = await model(false).complete({
    ...completion,
    tools: [
      {
        type: "function",
        function: { name: "finish", description: "finish", parameters: {} },
      },
    ],
    stopAfterTool: () => finished,
    executeTool: async () => {
      executed++;
      finished = true;
      return "finished";
    },
  });
  assert.equal(text, "");
  assert.equal(calls, 1);
  assert.equal(executed, 1);
});
