import assert from "node:assert/strict";
import { test } from "node:test";
import { MEMORY_TOOL } from "../src/memory.ts";
import { EXECUTION_TOOL } from "../src/execution.ts";
import { ModelClient, type ToolCall } from "../src/model.ts";
import { sessionReady, tickDecision, type TickSession } from "../src/tick.ts";
import { tickResponse } from "../src/tick-response.ts";

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
          assert.deepEqual(
            body.tools.map(
              (tool: { function: { name: string } }) => tool.function.name,
            ),
            ["finish_tick", "memory"],
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
  assert.throws(
    () => tickDecision({ messages: [], actions: [] }, []),
    /Expected argument object/,
  );
});

test("unknown arguments identify the tool and allowed fields without exposing arbitrary keys or values", () => {
  const secret = "private-unknown-field-and-value";
  assert.throws(
    () =>
      tickResponse(
        [call("execute_code", { code: "console.log(1)", [secret]: secret })],
        [EXECUTION_TOOL],
      ),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.match(
        error.message,
        /actions\[0\].arguments \(execute_code\): Unknown argument/,
      );
      assert.ok(
        error.message.includes('allowed fields: ["code","file","timeoutMs"]'),
      );
      assert.ok(!error.message.includes(secret));
      return true;
    },
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
