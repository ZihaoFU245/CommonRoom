import assert from "node:assert/strict";
import { test } from "node:test";
import { ModelClient, type ToolCall } from "../src/model.ts";
import { tickResponseShape } from "../src/model-diagnostics.ts";

const secret = "private-prompt-arguments-content-reasoning-token";
const decision = JSON.stringify({
  messages: [{ text: secret }],
  actions: [],
  next: { kind: "wait" },
  [secret]: secret,
});
const call = (name: string, args = decision, id = "call-1"): ToolCall => ({
  id,
  type: "function",
  function: { name, arguments: args },
});

test("tick response diagnostics expose structure without arbitrary names, fields or values", () => {
  const shape = JSON.parse(
    tickResponseShape(
      secret,
      [
        call("finish_tick"),
        call(secret, secret),
        call("memory", JSON.stringify({ [secret]: secret })),
      ],
      ["memory"],
    ),
  );
  assert.equal(shape.text_chars, secret.length);
  assert.equal(shape.tool_calls, 3);
  assert.deepEqual(shape.calls[0], {
    tool: "finish_tick",
    arguments_chars: decision.length,
    arguments_json: "object",
    messages: 1,
    actions: 0,
    next: "wait",
    unknown_fields: 1,
  });
  assert.equal(shape.calls[1].tool, "unavailable");
  assert.equal(shape.calls[1].arguments_json, "invalid_json");
  assert.equal(shape.calls[2].tool, "memory");
  assert.ok(!JSON.stringify(shape).includes(secret));
});

for (const stream of [false, true]) {
  for (const scenario of [
    "multiple_calls",
    "native_call",
    "unknown_call",
    "invalid_json",
    "invalid_decision",
    "final_text",
    "valid_decision",
  ]) {
    test(`tick diagnostics identify ${scenario} without payload leakage (stream=${stream})`, async (t) => {
      const logs: string[] = [];
      const calls =
        scenario === "multiple_calls"
          ? [call("finish_tick"), call("finish_tick", decision, "call-2")]
          : scenario === "native_call"
            ? [call("memory", JSON.stringify({ action: "get", key: secret }))]
            : scenario === "unknown_call"
              ? [call(secret)]
              : scenario === "invalid_json"
                ? [call("finish_tick", secret)]
                : scenario === "invalid_decision"
                  ? [call("finish_tick", decision)]
                  : scenario === "valid_decision"
                    ? [
                        call(
                          "finish_tick",
                          JSON.stringify({
                            messages: [{ text: secret }],
                            actions: [],
                            next: { kind: "wait" },
                          }),
                        ),
                      ]
                    : [];
      t.mock.method(
        globalThis,
        "fetch",
        async (_input: unknown, init: RequestInit) => {
          const body = JSON.parse(String(init.body));
          assert.equal(Object.hasOwn(body, "tickId"), false);
          return stream
            ? new Response(
                `data: ${JSON.stringify({ choices: [{ delta: { content: secret, reasoning_content: secret, tool_calls: calls.map((value, index) => ({ index, ...value })) }, finish_reason: "stop" }] })}\n\ndata: [DONE]\n\n`,
              )
            : Response.json({
                choices: [
                  {
                    message: {
                      content: secret,
                      reasoning_content: secret,
                      tool_calls: calls,
                    },
                    finish_reason: "stop",
                  },
                ],
              });
        },
      );
      const model = new ModelClient({
        endpoint: "https://model.invalid",
        model: "test-model",
        token: secret,
        stream,
        timeoutMs: 1000,
        log: (level, message) => logs.push(`${level} ${message}`),
      });
      const work = model.tick({
        messages: [{ role: "user", content: secret }],
        contextKey: "test-conversation",
        tickId: 9,
        tools: [
          {
            type: "function",
            function: {
              name: "memory",
              description: secret,
              parameters: { type: "object" },
            },
          },
        ],
      });
      if (
        scenario === "final_text" ||
        scenario === "valid_decision" ||
        scenario === "native_call"
      )
        await work;
      else await assert.rejects(work, /finish_tick decision/);
      const output = logs.join("\n");
      assert.match(
        output,
        /model tick request conversation=test-conversation tick=9/,
      );
      assert.match(
        output,
        /model tick response conversation=test-conversation tick=9/,
      );
      if (scenario === "multiple_calls")
        assert.match(
          output,
          /reason=unexpected_tool_calls count=2 tools=finish_tick,finish_tick/,
        );
      if (scenario === "native_call")
        assert.match(
          output,
          /model tick accepted.*format=native_tools.*actions=1 next=on_jobs/,
        );
      if (scenario === "unknown_call")
        assert.match(
          output,
          /reason=unexpected_tool_calls count=1 tools=unavailable/,
        );
      if (scenario === "invalid_json")
        assert.match(
          output,
          /reason=invalid_decision validation=finish_tick: expected valid JSON arguments/,
        );
      if (scenario === "invalid_decision")
        assert.match(
          output,
          /reason=invalid_decision validation=Expected messages, actions and next/,
        );
      if (scenario === "final_text")
        assert.match(output, /model tick accepted.*format=final_text/);
      if (scenario === "valid_decision")
        assert.match(output, /model tick accepted.*format=finish_tick/);
      assert.ok(!output.includes(secret));
    });
  }
}
