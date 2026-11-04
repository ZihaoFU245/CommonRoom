import assert from "node:assert/strict";
import test from "node:test";
import { sameMediaAuthority } from "../src/features/conversation/mediaAuthority.ts";

const snapshot = {
  sync: { instance: "one", revision: 1 },
  account_access: { id: "viewer" },
  policy_revision: 1,
  permissions: ["read"],
  users: ["alice"],
  rooms: [
    { id: "room", permissions: ["read"], members: ["alice"], messages: [] },
  ],
  private_access: { alice: { id: "private", permissions: ["read"] } },
};
test("media authority ignores content and presence but catches every access input", () => {
  const copy = () => structuredClone(snapshot);
  const unchanged = copy();
  unchanged.sync.revision++;
  unchanged.rooms[0].messages.push({ text: "new" });
  unchanged.online = ["alice"];
  assert.equal(sameMediaAuthority(snapshot, unchanged), true);
  for (const change of [
    (s) => {
      s.sync.instance = "two";
    },
    (s) => {
      s.account_access.id = "other";
    },
    (s) => {
      s.policy_revision++;
    },
    (s) => {
      s.permissions = [];
    },
    (s) => {
      s.users = [];
    },
    (s) => {
      s.rooms[0].id = "other";
    },
    (s) => {
      s.rooms[0].members = [];
    },
    (s) => {
      s.rooms[0].permissions = [];
    },
    (s) => {
      s.private_access.alice.permissions = [];
    },
    (s) => {
      delete s.private_access.alice;
    },
  ]) {
    const changed = copy();
    change(changed);
    assert.equal(sameMediaAuthority(snapshot, changed), false);
  }
});
