import assert from "node:assert/strict";
import test from "node:test";
import {
  timelineOffsets,
  virtualWindow,
  anchoredScrollTop,
  indexAt,
} from "../src/features/conversation/virtualTimeline.ts";
const keys = Array.from({ length: 1000 }, (_, i) => String(i));
test("1000 cached messages mount only the viewport and overscan", () => {
  const offsets = timelineOffsets(keys, new Map());
  for (const top of [0, 10000, 50000, 99400]) {
    const window = virtualWindow(offsets, top, 600);
    assert.ok(window.end - window.start <= 23);
    assert.equal(
      window.top +
        (offsets[window.end] - offsets[window.start]) +
        window.bottom,
      100000,
    );
    assert.ok(window.start <= indexAt(offsets, top));
    assert.ok(window.end > indexAt(offsets, Math.min(99999, top + 599)));
  }
  assert.deepEqual(virtualWindow([0], 0, 600), {
    start: 0,
    end: 0,
    top: 0,
    bottom: 0,
  });
});
test("variable heights locate exact boundaries and preserve an interior row anchor", () => {
  const oldKeys = ["a", "b", "c"];
  const offsets = timelineOffsets(
    oldKeys,
    new Map([
      ["a", 20],
      ["b", 300],
      ["c", 50],
    ]),
  );
  assert.deepEqual(
    [0, 19, 20, 319, 320, 369].map((p) => indexAt(offsets, p)),
    [0, 0, 1, 1, 2, 2],
  );
  const newKeys = ["older", ...oldKeys];
  const measured = timelineOffsets(
    newKeys,
    new Map([
      ["older", 70],
      ["a", 60],
      ["b", 300],
      ["c", 50],
    ]),
  );
  assert.equal(anchoredScrollTop(oldKeys, offsets, newKeys, measured, 45), 155);
});
test("evicting the opposite cache edge and resizing rows retain the visible message", () => {
  const offsets = timelineOffsets(keys, new Map());
  const nextKeys = keys
    .slice(100)
    .concat(Array.from({ length: 100 }, (_, i) => String(i + 1000)));
  const nextOffsets = timelineOffsets(nextKeys, new Map([["400", 250]]));
  assert.equal(
    anchoredScrollTop(keys, offsets, nextKeys, nextOffsets, 50020),
    40170,
  );
  assert.equal(
    anchoredScrollTop(keys, offsets, ["replacement"], [0, 100], 50020),
    50020,
  );
});
