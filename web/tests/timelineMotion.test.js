import assert from "node:assert/strict";
import test from "node:test";
import { timelineChanges } from "../src/features/conversation/timelineMotion.ts";

function message(sequence, text = "hello", extra = {}) {
  return {
    kind: "message",
    key: String(sequence),
    order: sequence,
    message: { id: String(sequence), sequence, text, ...extra },
  };
}

test("live arrivals enter while fetched older messages do not", () => {
  const before = [message(100), message(200)];
  const changes = timelineChanges(before, [
    message(50),
    ...before,
    message(300),
    message(400),
  ]);
  assert.deepEqual([...changes.added], ["300", "400"]);
  assert.equal(changes.edited.size, 0);
  assert.equal(changes.removed.size, 0);
});

test("retractions and remote text edits share a layout transition", () => {
  const before = [message(1), message(2), message(3)];
  const changes = timelineChanges(before, [
    message(1, "edited\nwith more lines", { edited: true }),
    message(3),
  ]);
  assert.deepEqual([...changes.edited], ["1"]);
  assert.deepEqual([...changes.removed], ["2"]);
  assert.equal(changes.added.size, 0);
});

test("reaction, read and local command updates do not replay message text", () => {
  const before = [message(1, "hello", { reactions: {} })];
  const changes = timelineChanges(before, [
    message(1, "hello", { markdown: false, reactions: { "👍": ["bob"] } }),
    { kind: "notice", key: "local", order: 2, text: "Done" },
  ]);
  assert.equal(
    changes.added.size + changes.edited.size + changes.removed.size,
    0,
  );
});

test("a last-message deletion and formatting changes are detected", () => {
  const changes = timelineChanges(
    [message(1), message(2)],
    [message(1, "hello", { markdown: true })],
  );
  assert.deepEqual([...changes.edited], ["1"]);
  assert.deepEqual([...changes.removed], ["2"]);
});
