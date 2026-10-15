import assert from "node:assert/strict";
import { test } from "node:test";
import { ModelClient } from "../src/model.ts";

const secret = "private-provider-body-or-credential";
const request = {
  messages: [{ role: "user" as const, content: secret }],
  maxTokens: 128,
};
function client(stream: boolean, logs: string[]): ModelClient {
  return new ModelClient({
    endpoint: "https://model.invalid/",
    token: secret,
    model: "ds-zf/deepseek-flash",
    timeoutMs: 1000,
    stream,
    log: (level, message) => logs.push(`${level} ${message}`),
  });
}

for (const stream of [true, false]) {
  for (const status of [401, 403, 404]) {
    test(`upstream denial logs an error immediately without retry or body leakage (stream=${stream}, status=${status})`, async (t) => {
      const logs: string[] = [];
      let attempts = 0;
      t.mock.method(globalThis, "fetch", async () => {
        attempts++;
        return Response.json({ error: { message: secret } }, { status });
      });
      await assert.rejects(
        client(stream, logs).complete(request),
        new RegExp(`HTTP ${status}`),
      );
      assert.equal(attempts, 1);
      assert.ok(
        logs.some(
          (line) =>
            line.startsWith("debug model request started") &&
            line.includes('model="ds-zf/deepseek-flash"'),
        ),
      );
      assert.ok(
        logs.some(
          (line) =>
            line.startsWith("error model upstream rejected") &&
            line.includes(`status=${status}`),
        ),
      );
      assert.ok(
        logs.some(
          (line) =>
            line.startsWith("error model request failed") &&
            line.includes(`status=${status}`),
        ),
      );
      assert.ok(!logs.join("\n").includes(secret));
    });
  }
  for (const refusal of [true, false]) {
    test(`structured refusals and content filters are logged and never published (stream=${stream}, refusal=${refusal})`, async (t) => {
      const logs: string[] = [];
      let attempts = 0;
      t.mock.method(globalThis, "fetch", async () => {
        attempts++;
        const choice = stream
          ? {
              delta: refusal ? { refusal: secret } : {},
              finish_reason: refusal ? "stop" : "content_filter",
            }
          : {
              message: refusal
                ? { refusal: secret, content: null }
                : { content: secret },
              finish_reason: refusal ? "stop" : "content_filter",
            };
        return stream
          ? new Response(
              `data: ${JSON.stringify({ choices: [choice] })}\n\ndata: [DONE]\n\n`,
            )
          : Response.json({ choices: [choice] });
      });
      await assert.rejects(
        client(stream, logs).complete(request),
        refusal ? /provider refused/ : /content_filter/,
      );
      assert.equal(attempts, 1);
      assert.ok(
        logs.some(
          (line) =>
            line.startsWith("error model request failed") &&
            line.includes("status=403"),
        ),
      );
      assert.ok(!logs.join("\n").includes(secret));
    });
  }
}

test("debug traces request, first delta, tool loop and usage without logging payloads", async (t) => {
  const logs: string[] = [];
  let attempts = 0;
  t.mock.method(globalThis, "fetch", async () => {
    attempts++;
    const choice =
      attempts === 1
        ? {
            delta: {
              tool_calls: [
                {
                  index: 0,
                  id: "call-1",
                  type: "function",
                  function: {
                    name: "read_history",
                    arguments: JSON.stringify({ value: secret }),
                  },
                },
              ],
            },
            finish_reason: "tool_calls",
          }
        : { delta: { content: secret }, finish_reason: "stop" };
    return new Response(
      `data: ${JSON.stringify({ choices: [choice] })}\n\ndata: ${JSON.stringify({ choices: [], usage: { prompt_tokens: 100, completion_tokens: 8, prompt_cache_hit_tokens: 60 } })}\n\ndata: [DONE]\n\n`,
    );
  });
  const answer = await client(true, logs).complete({
    ...request,
    tools: [
      {
        type: "function",
        function: {
          name: "read_history",
          description: "history",
          parameters: { type: "object" },
        },
      },
    ],
    executeTool: async () => secret,
  });
  assert.equal(answer, secret);
  const output = logs.join("\n");
  assert.match(output, /model HTTP response status=200/);
  assert.match(output, /model stream first delta kind=tool_call/);
  assert.match(output, /model stream first delta kind=text/);
  assert.match(output, /model tool started.*tool=read_history/);
  assert.match(output, /model tool finished.*result_chars=/);
  assert.match(
    output,
    /model request finished.*input_tokens=100.*cached_tokens=60/,
  );
  assert.ok(!output.includes(secret));
});
