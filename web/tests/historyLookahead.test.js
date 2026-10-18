import assert from "node:assert/strict";
import test from "node:test";
import { HistoryLookahead } from "../src/features/conversation/historyLookahead.ts";
test("slow requests and fast scrolling start bounded paging before the edge", () => {
  const lookahead = new HistoryLookahead();
  lookahead.sample(2000);
  assert.equal(lookahead.observe(2000, 600, 0).direction, 0);
  const up = lookahead.observe(1800, 600, 50);
  assert.equal(up.direction, -1);
  assert.ok(up.distance > 600);
  for (let i = 0; i < 30; i++) lookahead.sample(15000);
  const fast = lookahead.observe(400, 600, 100);
  assert.equal(fast.distance, 1800);
  lookahead.reset();
  assert.equal(lookahead.observe(0, 600, 200).direction, 0);
});
