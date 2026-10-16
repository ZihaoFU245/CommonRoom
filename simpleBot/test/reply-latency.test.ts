import assert from "node:assert/strict";
import { test } from "node:test";
import { Metrics } from "../src/metrics.ts";

test("latency includes queue, multiple calls and final multipart acknowledgement exactly once", () => {
  const metrics = new Metrics();
  metrics.admit("a", 0);
  metrics.admit("b", 10);
  metrics.dequeued("a", 20);
  metrics.dequeued("a", 25); // A repair must not count the first queue wait twice.
  metrics.validated(15, 2, 1);
  metrics.prepared(10);
  metrics.record({ success: false, durationMs: 30 }, ["a", "b"]);
  metrics.record({ success: true, durationMs: 40, firstDeltaMs: 12 }, [
    "a",
    "b",
  ]);
  metrics.committed(5);
  metrics.planReply(["part-1", "part-2"], ["a", "b"], 100);
  // Later background work must not add steps to an already committed answer.
  metrics.record({ success: true, durationMs: 5 }, ["a", "b"]);
  metrics.deliveryStarted("part-1", 110);
  metrics.delivered("part-1", 130);
  assert.match(metrics.report(), /End-to-end reply \| unknown/);
  metrics.deliveryStarted("part-2", 140);
  metrics.delivered("part-2", 160);
  metrics.delivered("part-2", 170);
  const report = metrics.report();
  assert.match(
    report,
    /End-to-end reply \| 150 ms \| 155 ms \| 160 ms \| 2 \|/,
  );
  assert.match(
    report,
    /Queue \(includes batching\) \| 20 ms \| 20 ms \| 20 ms \| 1 \|/,
  );
  assert.match(report, /Delivery queue \| 10 ms \| 10 ms \| 10 ms \| 1 \|/);
  assert.match(report, /Delivery \| 50 ms \| 50 ms \| 50 ms \| 1 \|/);
  assert.match(
    report,
    /Model calls per answered input \| 2\.00 \(2 inputs measured/,
  );
  assert.match(report, /Source validation \| 2 inputs \/ 1 HTTP reads/);
  assert.ok(!report.includes("part-1"));
});

test("restarts, edits, local controls and discarded inputs do not invent total latency", () => {
  const metrics = new Metrics();
  metrics.planReply(["recovered"], ["unknown-input"], 10);
  metrics.deliveryStarted("recovered", 20);
  metrics.delivered("recovered", 30);
  metrics.admit("edited", 0);
  metrics.planReply(["old-version"], ["edited"], 10);
  metrics.admit("edited", 20);
  metrics.delivered("old-version", 50);
  metrics.admit("control", 0);
  metrics.forgetInput("control");
  metrics.planReply(["control-part"], ["control"], 10);
  metrics.delivered("control-part", 50);
  assert.match(metrics.report(), /End-to-end reply \| unknown/);
  assert.match(metrics.report(), /Model calls per answered input \| unknown/);
});

test("metrics render bounded percentile samples and numeric public limits as Markdown tables", () => {
  const metrics = new Metrics();
  for (let index = 0; index < 200; index++) metrics.prepared(index);
  const report = metrics.report({
    compactTokens: 24576,
    contextTokens: 32768,
    batchWindowMs: 1000,
  });
  assert.match(
    report,
    /Context preparation \| 199 ms \| 100 ms \| 193 ms \| 200 \|/,
  );
  assert.ok(report.includes("| Stage | Latest | Mean | P95 | Samples |"));
  assert.ok(report.includes("| Autonomous collection window | 1000 ms |"));
  assert.ok(report.length < 4000);
});
