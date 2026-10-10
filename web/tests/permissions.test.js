import assert from "node:assert/strict";
import test from "node:test";
import { messageActions } from "../src/features/conversation/permissions.ts";
const commands = ["/react", "/reply", "/retract"].map((name) => ({ name }));
const other = { from: "bob" };
test("read-only access never exposes mutating controls, even with command grants", () => {
  assert.deepEqual(
    messageActions(other, "alice", ["r:message.read"], commands),
    { react: false, reply: false, retract: false },
  );
});
test("actions require both the scoped command and resource grant", () => {
  assert.deepEqual(
    messageActions(
      other,
      "alice",
      ["w:message.react", "w:message.create", "w:message.retract.any"],
      [],
    ),
    { react: false, reply: false, retract: false },
  );
});
test("own retraction cannot delete another author; su any-message grants can", () => {
  assert.equal(
    messageActions(other, "alice", ["w:message.retract.own"], commands).retract,
    false,
  );
  assert.equal(
    messageActions(other, "bob", ["w:message.retract.own"], commands).retract,
    true,
  );
  assert.equal(
    messageActions(other, "alice", ["w:message.retract.any"], commands).retract,
    true,
  );
});
