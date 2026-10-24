import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  groupedMessage,
  sameMinuteMessage,
  messageDate,
} from "../src/features/conversation/messages.ts";

test("invalid timestamps have a safe rendering fallback", () => {
  for (const time of [1e15, Infinity, NaN, -Infinity]) {
    assert.equal(messageDate(time).label, "Unknown date");
    assert.equal(messageDate(time).iso, "");
  }
  assert.equal(
    messageDate(8_640_000_000_000).iso,
    "+275760-09-13T00:00:00.000Z",
  );
});

test("message dates follow local timezone including midnight and DST", () => {
  const module = new URL(
    "../src/features/conversation/messages.ts",
    import.meta.url,
  ).href;
  const run = (tz, timestamp) =>
    JSON.parse(
      execFileSync(
        process.execPath,
        [
          "--input-type=module",
          "-e",
          `import {messageDate} from ${JSON.stringify(module)}; console.log(JSON.stringify(messageDate(${timestamp})));`,
        ],
        { env: { ...process.env, TZ: tz }, encoding: "utf8" },
      ),
    );
  const timestamp = Date.parse("2026-10-07T18:30:00Z") / 1000;
  assert.equal(run("UTC", timestamp).time, "18:30");
  const hk = run("Asia/Hong_Kong", timestamp);
  assert.equal(hk.time, "02:30");
  assert.equal(hk.key, "2026-9-8");
  assert.equal(hk.iso, "2026-10-07T18:30:00.000Z");
  assert.equal(
    run("America/New_York", Date.parse("2026-03-08T06:59:00Z") / 1000).time,
    "01:59",
  );
  assert.equal(
    run("America/New_York", Date.parse("2026-03-08T07:01:00Z") / 1000).time,
    "03:01",
  );
});
test("grouping breaks for another sender, backwards time, long gaps and local midnight", () => {
  const time = new Date(2026, 9, 8, 12, 0).getTime() / 1000;
  const previous = { from: "alice", time };
  assert.equal(
    groupedMessage({ ...previous, time: time + 60 }, previous),
    true,
  );
  assert.equal(
    groupedMessage({ from: "bob", time: time + 60 }, previous),
    false,
  );
  assert.equal(
    groupedMessage({ ...previous, time: time - 1 }, previous),
    false,
  );
  assert.equal(
    groupedMessage({ ...previous, time: time + 300 }, previous),
    false,
  );
  assert.equal(groupedMessage(previous, null), false);
  const midnight = new Date(2026, 9, 9).getTime() / 1000;
  assert.equal(
    groupedMessage(
      { from: "alice", time: midnight },
      { from: "alice", time: midnight - 60 },
    ),
    false,
  );
});

test("message bars connect only consecutive messages in the same author minute and conversation", () => {
  const time = new Date(2026, 9, 21, 16, 44, 0).getTime() / 1000;
  const previous = { from: "alice", time: time + 5, to: null };
  assert.equal(
    sameMinuteMessage({ ...previous, time: time + 59 }, previous),
    true,
  );
  assert.equal(
    sameMinuteMessage(
      { ...previous, time: time + 60 },
      { ...previous, time: time + 59 },
    ),
    false,
  );
  assert.equal(
    sameMinuteMessage({ ...previous, from: "bob" }, previous),
    false,
  );
  assert.equal(
    sameMinuteMessage({ ...previous, time: time + 4 }, previous),
    false,
  );
  assert.equal(sameMinuteMessage(previous, null), false);
  assert.equal(
    sameMinuteMessage(
      { ...previous, private_id: "other" },
      { ...previous, private_id: "pair" },
    ),
    false,
  );
  assert.equal(
    sameMinuteMessage({ ...previous, to: "eve" }, { ...previous, to: "bob" }),
    false,
  );
  const midnight = new Date(2026, 9, 22).getTime() / 1000;
  assert.equal(
    sameMinuteMessage(
      { ...previous, time: midnight },
      { ...previous, time: midnight - 1 },
    ),
    false,
  );
});
