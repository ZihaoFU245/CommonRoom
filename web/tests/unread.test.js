import { test } from "node:test";
import assert from "node:assert/strict";
import {
  retainedHistory,
  visibleReadPosition,
  badgeLabel,
} from "../src/features/conversation/unread.ts";
import { ingest, timeline } from "../src/features/conversation/console.ts";
test("history backfill preserves message and local command order", () => {
  const newer = { id: "3", sequence: 3, time: 3 };
  const snapshot = { rooms: [{ messages: [newer] }], direct: [] };
  const ledger = ingest(snapshot);
  const output = [{ key: "command", order: ++ledger.sequence, room: "room" }];
  const older = [
    { id: "1", sequence: 1, time: 1 },
    { id: "2", sequence: 2, time: 2 },
  ];
  ingest(snapshot, ledger, older);
  assert.deepEqual(
    timeline([...older, newer], output, ledger, 0, "room").map((e) => e.key),
    ["1", "2", "3", "command"],
  );
  assert.equal(
    timeline([...older, newer], output, ledger, ledger.sequence, "room").length,
    0,
  );
  const live = { id: "4", sequence: 4, time: 4 };
  ingest({ rooms: [{ messages: [newer, live] }], direct: [] }, ledger, older);
  assert.deepEqual(
    timeline([...older, newer, live], output, ledger, 0, "room").map(
      (e) => e.key,
    ),
    ["1", "2", "3", "command", "4"],
  );
});
test("retention evicts stale history and updates reactions without duplication", () => {
  const history = [1, 2, 3].map((sequence) => ({
    id: String(sequence),
    sequence,
  }));
  const tail = [
    { id: "3", sequence: 3, reactions: { "👍": ["bob"] } },
    { id: "4", sequence: 4 },
  ];
  const result = retainedHistory(history, tail, { oldest: 2, through: 4 });
  assert.deepEqual(
    result.map((m) => m.sequence),
    [2, 3, 4],
  );
  assert.deepEqual(result[1].reactions, { "👍": ["bob"] });
});
test("read acknowledgement requires loaded unread history and visible messages", () => {
  const viewport = { top: 0, bottom: 100 };
  const elements = [
    { sequence: 51, top: 0, bottom: 30 },
    { sequence: 52, top: 30, bottom: 70 },
    { sequence: 53, top: 70, bottom: 120 },
  ];
  assert.equal(
    visibleReadPosition(elements, viewport, 1),
    0,
    "do not skip unread history outside snapshot tail",
  );
  assert.equal(visibleReadPosition(elements, viewport, 51), 52);
  assert.equal(
    visibleReadPosition(elements, viewport, 53),
    0,
    "below viewport is unread",
  );
  assert.equal(visibleReadPosition(elements, viewport, null), 52);
  assert.equal(badgeLabel(120), "99+");
  assert.equal(badgeLabel(4), "4");
});
