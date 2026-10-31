import assert from "node:assert/strict";
import test from "node:test";
import {
  resolveDisplayNames,
  mergeDisplayNames,
} from "../src/features/preferences/displayNames.ts";
import { isSnapshot, isHistory, parseFrame } from "../src/api/protocol.ts";

test("display names retain duplicates and reject stale page names", () => {
  assert.deepEqual(
    resolveDisplayNames({ alice: "Same", bob: "Same" }, "2", {
      version: "1",
      values: { bob: "Old" },
    }),
    { alice: "Same", bob: "Same" },
  );
  assert.deepEqual(
    resolveDisplayNames({ alice: "Same", bob: "Same" }, "2", {
      version: "2",
      values: { bob: "Room Bob", alice: null },
    }),
    { bob: "Room Bob" },
  );
});
test("paged display names stay bounded and clear old versions", () => {
  const messages = [{ from: "bob", mentions: [], reply: { from: "alice" } }];
  assert.deepEqual(
    mergeDisplayNames(
      { version: "1", values: { eve: "Hidden", bob: "Old" } },
      { version: "2", values: { bob: "New", alice: null } },
      messages,
    ),
    { version: "2", values: { bob: "New", alice: null } },
  );
});
test("malformed display name projections are rejected", () => {
  assert.equal(isSnapshot({ display_names: { bob: 17 } }), false);
  const page = {
    view: "lobby",
    messages: [],
    revision: 1,
    has_before: false,
    has_after: false,
  };
  assert.equal(
    isHistory({
      ...page,
      message_display_names: { version: "1", values: { bob: "Bob Smith" } },
    }),
    true,
  );
  assert.equal(
    isHistory({
      ...page,
      message_display_names: { version: "1", values: { bob: 17 } },
    }),
    false,
  );
  assert.equal(
    parseFrame(
      JSON.stringify({
        kind: "conversation",
        view: "lobby",
        id: "room",
        messages: [],
        unread: { count: 0, first: null, through: 0, oldest: 0, revision: 1 },
        message_display_names: { version: "1", values: { bob: 17 } },
      }),
    ),
    null,
  );
});
