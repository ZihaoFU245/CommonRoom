import assert from "node:assert/strict";
import { test } from "node:test";
import { MEMORY_TOOL } from "../src/memory.ts";
import { EXECUTION_TOOL } from "../src/execution.ts";
import { ModelClient, type ToolCall } from "../src/model.ts";
import { sessionReady, tickDecision, type TickSession } from "../src/tick.ts";
import { tickResponse } from "../src/tick-response.ts";
import type { FunctionTool } from "../src/model.ts";

const action = {
  id: "lookup",
  tool: "memory",
  arguments: { action: "get", key: "index" },
};
const call = (name: string, args: unknown, id = "call-1"): ToolCall => ({
  id,
  type: "function",
  function: { name, arguments: JSON.stringify(args) },
});
const envelope = (actions = [action], jobs = actions.map((item) => item.id)) =>
  call(
    "finish_tick",
    {
      messages: [],
      actions,
      next: { kind: "on_jobs", jobs, mode: "all" },
    },
    "envelope",
  );

test("reply IDs and limits do not change provider tools or discard token measurements", async (t) => {
  const advertised: FunctionTool[][] = [];
  const sources: string[] = [];
  t.mock.method(
    globalThis,
    "fetch",
    async (_url: unknown, init: RequestInit) => {
      const body = JSON.parse(String(init.body));
      advertised.push(body.tools);
      return Response.json({
        choices: [{ message: { content: "" }, finish_reason: "stop" }],
        usage: { prompt_tokens: 100, completion_tokens: 0 },
      });
    },
  );
  const model = new ModelClient({
    endpoint: "https://model.invalid",
    model: "test",
    token: "",
    stream: false,
    timeoutMs: 1000,
    log: () => {},
  });
  for (const replyTargets of [["input-a"], ["input-b", "input-c"], []])
    await model.tick({
      messages: [{ role: "user", content: "same retained prefix" }],
      tools: [MEMORY_TOOL],
      contextKey: "conversation",
      replyTargets,
      maxReplyMessages: replyTargets.length,
      onCall: (call) => sources.push(call.inputBudget?.source ?? "unknown"),
    });
  assert.deepEqual(advertised[0], advertised[1]);
  assert.deepEqual(advertised[1], advertised[2]);
  assert.deepEqual(sources, ["estimate", "provider", "provider"]);
  assert.ok(!JSON.stringify(advertised).includes("input-a"));
});

for (const stream of [false, true]) {
  for (const mixed of [false, true]) {
    test(`native calls become durable intents without inline effects (stream=${stream}, mixed=${mixed})`, async (t) => {
      let requests = 0;
      const native = call("memory", { key: "index", action: "get" });
      const calls = mixed ? [native, envelope()] : [native];
      t.mock.method(
        globalThis,
        "fetch",
        async (_url: unknown, init: RequestInit) => {
          requests++;
          const body = JSON.parse(String(init.body));
          // One envelope only: actions are batched, so a tick never spends a
          // separate inference per tool. Every tool is still advertised, nested
          // in the envelope's action schema.
          assert.deepEqual(
            body.tools.map(
              (tool: { function: { name: string } }) => tool.function.name,
            ),
            ["finish_tick"],
          );
          assert.deepEqual(
            body.tools[0].function.parameters.properties.actions.items.anyOf.map(
              (option: { properties: { tool: { enum: string[] } } }) =>
                option.properties.tool.enum[0],
            ),
            ["memory"],
          );
          assert.equal(
            body.tools[0].function.parameters.properties.actions.items.anyOf[0]
              .description,
            MEMORY_TOOL.function.description,
          );
          return stream
            ? new Response(
                `data: ${JSON.stringify({ choices: [{ delta: { content: "I have finished", tool_calls: calls.map((item, index) => ({ index, ...item })) }, finish_reason: "tool_calls" }] })}\n\ndata: [DONE]\n\n`,
              )
            : Response.json({
                choices: [
                  {
                    message: { content: "I have finished", tool_calls: calls },
                    finish_reason: "tool_calls",
                  },
                ],
              });
        },
      );
      const model = new ModelClient({
        endpoint: "https://model.invalid",
        model: "test",
        token: "",
        stream,
        timeoutMs: 1000,
        log: () => {},
      });
      const result = await model.tick({
        messages: [{ role: "user", content: "look up index" }],
        tools: [MEMORY_TOOL],
        executeTool: () => {
          throw new Error("Must wait for a durable commit");
        },
      });
      assert.equal(requests, 1);
      assert.deepEqual(result.messages, []);
      assert.equal(result.actions.length, 1);
      assert.deepEqual(result.actions[0].arguments, action.arguments);
      assert.deepEqual(result.next, {
        kind: "on_jobs",
        jobs: [result.actions[0].id],
        mode: "all",
      });
    });
  }
}

test("mixed calls preserve independent jobs, coalesce duplicate representations and map native references", () => {
  const extra = call("memory", { action: "get", key: "other" }, "other-call");
  const result = tickResponse(
    [
      call("memory", action.arguments),
      extra,
      envelope([action], ["call-1", "lookup", "other-call"]),
    ],
    [MEMORY_TOOL],
  );
  assert.equal(result.coalesced, 1);
  assert.equal(result.decision.actions.length, 2);
  assert.deepEqual(result.decision.next, {
    kind: "on_jobs",
    jobs: ["lookup", result.decision.actions[1].id],
    mode: "all",
  });
});

test("identical but separate native calls retain distinct intent IDs", () => {
  const result = tickResponse(
    [
      call("memory", action.arguments),
      call("memory", action.arguments, "call-2"),
      envelope(),
    ],
    [MEMORY_TOOL],
  );
  assert.equal(result.coalesced, 1);
  assert.equal(result.decision.actions.length, 2);
  assert.notEqual(result.decision.actions[0].id, result.decision.actions[1].id);
});

for (const stray of ["envelope", "native", "both"]) {
  test(`mixed effects coalesce after argument normalization (stray=${stray})`, () => {
    const args = { code: "console.log(1)" };
    const result = tickResponse(
      [
        call(
          "finish_tick",
          {
            actions: [
              {
                id: "run",
                tool: "execute_code",
                arguments: {
                  ...args,
                  ...(stray !== "native" ? { language: "typescript" } : {}),
                },
              },
            ],
            next: { kind: "on_jobs", jobs: ["call-1", "run"], mode: "all" },
          },
          "envelope",
        ),
        call("execute_code", {
          ...args,
          ...(stray !== "envelope" ? { language: "typescript" } : {}),
        }),
        // Matching one envelope representation must not merge two native intents.
        call("execute_code", { ...args, language: "typescript" }, "call-2"),
      ],
      [EXECUTION_TOOL],
    );
    assert.equal(result.coalesced, 1);
    assert.equal(result.decision.actions.length, 2);
    assert.equal(result.decision.actions[0].id, "run");
    assert.notEqual(result.decision.actions[1].id, "run");
    for (const action of result.decision.actions)
      assert.deepEqual(action.arguments, args);
    assert.deepEqual(result.decision.next, {
      kind: "on_jobs",
      jobs: ["run"],
      mode: "all",
    });
    assert.deepEqual([...new Set(result.normalized)], ["execute_code"]);
  });
}

test("native IDs cannot ambiguously alias another envelope action", () => {
  assert.throws(
    () =>
      tickResponse(
        [call("memory", { action: "get", key: "other" }, "lookup"), envelope()],
        [MEMORY_TOOL],
      ),
    /ambiguous continuation ID/,
  );
});

test("omitted empty lists are normalized without accepting invalid explicit lists", () => {
  assert.deepEqual(
    tickResponse([call("finish_tick", { next: { kind: "wait" } })], [])
      .decision,
    { messages: [], actions: [], next: { kind: "wait" } },
  );
  const result = tickResponse(
    [
      call("memory", action.arguments),
      call(
        "finish_tick",
        {
          messages: [],
          next: { kind: "on_jobs", jobs: ["call-1"], mode: "all" },
        },
        "finish",
      ),
    ],
    [MEMORY_TOOL],
  );
  assert.equal(result.decision.actions.length, 1);
  assert.deepEqual(result.decision.next, {
    kind: "on_jobs",
    jobs: [result.decision.actions[0].id],
    mode: "all",
  });
  assert.throws(
    () => tickDecision({ actions: null, next: { kind: "wait" } }, []),
    /Expected messages, actions and next/,
  );
  // A missing or shorthand continuation stops the episode instead of spending a
  // whole repair call. The host still learns the payload deviated.
  const normalized: string[] = [];
  assert.deepEqual(
    tickDecision({ messages: [], actions: [] }, [], normalized).next,
    { kind: "wait" },
  );
  assert.deepEqual(normalized, ["next"]);
  assert.deepEqual(
    tickDecision({ messages: [], actions: [], next: "continue" }, []).next,
    { kind: "continue" },
  );
  assert.throws(
    () => tickDecision({ messages: [], actions: [], next: "later" }, []),
    /Invalid continuation arguments/,
  );
});

test("unknown arguments are dropped with a host-owned label instead of failing the tick", () => {
  const secret = "private-unknown-field-and-value";
  const normalized: string[] = [];
  const decision = tickDecision(
    {
      messages: [],
      actions: [
        {
          id: "run",
          tool: "execute_code",
          arguments: { code: "console.log(1)", [secret]: secret },
        },
      ],
      next: { kind: "wait" },
      [secret]: secret,
    },
    [EXECUTION_TOOL],
    normalized,
  );
  assert.deepEqual(decision.actions[0]!.arguments, { code: "console.log(1)" });
  assert.deepEqual([...new Set(normalized)].sort(), [
    "execute_code",
    "finish_tick",
  ]);
  assert.ok(!JSON.stringify(normalized).includes(secret));
});

test("a double-encoded or fenced decision is repaired instead of rejected", () => {
  const value = {
    messages: [{ text: "batched" }],
    actions: [],
    next: { kind: "wait" },
  };
  for (const argumentsText of [
    JSON.stringify(JSON.stringify(value)),
    "```json\n" + JSON.stringify(value) + "\n```",
    `Here is the plan:\n${JSON.stringify(value)}\n`,
  ])
    assert.deepEqual(
      tickResponse(
        [
          {
            id: "envelope",
            type: "function",
            function: { name: "finish_tick", arguments: argumentsText },
          },
        ],
        [],
      ).decision.messages,
      [{ text: "batched" }],
    );
  assert.throws(
    () =>
      tickResponse(
        [
          {
            id: "envelope",
            type: "function",
            function: { name: "finish_tick", arguments: "not json at all" },
          },
        ],
        [],
      ),
    /finish_tick: expected valid JSON arguments/,
  );
});

test("invalid native arguments reject a mixed plan before any effect", () => {
  assert.throws(
    () =>
      tickResponse(
        [envelope(), call("memory", { action: "not-an-action" })],
        [MEMORY_TOOL],
      ),
    /actions\[1\].arguments/,
  );
});

test("action IDs follow advertised string bounds and duplicate errors identify the field", () => {
  for (const id of ["step:1", "查找", "😀".repeat(64), "__proto__"])
    assert.equal(
      tickDecision(
        { messages: [], actions: [{ ...action, id }], next: { kind: "wait" } },
        [MEMORY_TOOL],
      ).actions[0].id,
      id,
    );
  for (const id of ["", "😀".repeat(65)])
    assert.throws(
      () =>
        tickDecision(
          {
            messages: [],
            actions: [{ ...action, id }],
            next: { kind: "wait" },
          },
          [MEMORY_TOOL],
        ),
      /actions\[0\].id: expected 1 to 64 characters/,
    );
  assert.throws(
    () =>
      tickDecision(
        { messages: [], actions: [action, action], next: { kind: "wait" } },
        [MEMORY_TOOL],
      ),
    /actions\[1\].id: duplicate action ID/,
  );
});

for (const stream of [false, true])
  test(`validation feedback preserves a safe repair path (stream=${stream})`, async (t) => {
    const logs: string[] = [];
    const invalid = envelope([action, action]);
    t.mock.method(globalThis, "fetch", async () =>
      stream
        ? new Response(
            `data: ${JSON.stringify({ choices: [{ delta: { tool_calls: [{ index: 0, ...invalid }] }, finish_reason: "tool_calls" }] })}\n\ndata: [DONE]\n\n`,
          )
        : Response.json({
            choices: [
              {
                message: { content: null, tool_calls: [invalid] },
                finish_reason: "tool_calls",
              },
            ],
          }),
    );
    const model = new ModelClient({
      endpoint: "https://model.invalid",
      model: "test",
      token: "",
      stream,
      timeoutMs: 1000,
      log: (_level, text) => logs.push(text),
    });
    await assert.rejects(
      model.tick({
        messages: [{ role: "user", content: "private input" }],
        tools: [MEMORY_TOOL],
      }),
      /Invalid finish_tick decision: actions\[1\].id: duplicate action ID/,
    );
    assert.ok(!logs.join("\n").includes("private input"));
    assert.ok(!logs.join("\n").includes('"index"'));
  });

test("job readiness requires a fresh terminal result and respects any/all watches", () => {
  const session: TickSession = {
    root: "input",
    generation: 0,
    tick: 1,
    calls: 1,
    started: Date.now(),
    phase: "idle",
    next: { kind: "on_jobs", jobs: ["old", "pending"], mode: "any" },
    sources: [],
    trace: [],
    jobs: {
      old: {
        id: "old",
        tool: "memory",
        arguments: "{}",
        created: 0,
        status: "completed",
        observed: true,
      },
      pending: {
        id: "pending",
        tool: "memory",
        arguments: "{}",
        created: 0,
        status: "running",
      },
    },
  };
  assert.equal(sessionReady(session), false);
  session.jobs.pending.status = "completed";
  assert.equal(sessionReady(session), true);
  session.next = {
    kind: "on_jobs",
    jobs: ["old", "pending", "missing"],
    mode: "all",
  };
  assert.equal(sessionReady(session), false);
  session.next = { kind: "on_jobs", jobs: ["old", "pending"], mode: "all" };
  assert.equal(sessionReady(session), true);
  session.jobs.pending.observed = true;
  assert.equal(sessionReady(session), false);
});
