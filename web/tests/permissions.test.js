import assert from "node:assert/strict";
import test from "node:test";
import {
  debugAccess,
  messageActionCommand,
  messageActions,
} from "../src/features/conversation/permissions.ts";
const commands = ["/react", "/reply", "/retract", "/edit"].map((name) => ({
  name,
}));
const sudoCommands = [
  ...commands,
  {
    name: "/sudo",
    subcommands: ["/edit", "/retract"].map((name) => ({ name })),
  },
];
const other = { from: "bob" };
test("sudo debug needs an authorized toggle and stops when either global sudo grant is revoked", () => {
  const permissions = ["r:message.read"];
  const globalPermissions = ["x:command.sudo"];
  const globalCommands = [{ name: "/sudo" }];
  assert.equal(
    debugAccess(permissions, false, globalPermissions, globalCommands),
    false,
  );
  assert.equal(
    debugAccess(permissions, true, globalPermissions, globalCommands),
    true,
  );
  assert.equal(debugAccess(permissions, true, [], globalCommands), false);
  assert.equal(debugAccess(permissions, true, globalPermissions, []), false);
  assert.equal(debugAccess(["r:message.metadata"], false, [], []), true);
});
test("read-only access never exposes mutating controls, even with command grants", () => {
  assert.deepEqual(
    messageActions(other, "alice", ["r:message.read"], commands),
    { react: false, reply: false, edit: false, retract: false },
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
    { react: false, reply: false, edit: false, retract: false },
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

test("editing requires ownership and both scoped grants", () => {
  const permission = ["w:message.edit.own"];
  assert.equal(messageActions(other, "bob", permission, commands).edit, true);
  assert.equal(
    messageActions(other, "alice", permission, commands).edit,
    false,
  );
  assert.equal(messageActions(other, "bob", permission, []).edit, false);
  assert.equal(messageActions(other, "bob", [], commands).edit, false);
});

test("edit-any permits other authors but still requires the command grant", () => {
  assert.equal(
    messageActions(other, "alice", ["w:message.edit.any"], commands).edit,
    true,
  );
  assert.equal(
    messageActions(other, "alice", ["w:message.edit.any"], []).edit,
    false,
  );
  assert.equal(
    messageActions(other, "alice", ["w:message.retract.any"], commands).edit,
    false,
  );
});

test("sudo advertises edit and delete on every message without direct grants", () => {
  assert.deepEqual(messageActions(other, "alice", [], sudoCommands), {
    react: false,
    reply: false,
    edit: true,
    retract: true,
  });
});

test("sudo subcommands work without top-level message command grants", () => {
  const elevatedOnly = [
    ...commands.filter(
      (command) => !["/edit", "/retract"].includes(command.name),
    ),
    {
      name: "/sudo",
      subcommands: [{ name: "/edit" }, { name: "/retract" }],
    },
  ];
  assert.deepEqual(messageActions(other, "alice", [], elevatedOnly), {
    react: false,
    reply: false,
    edit: true,
    retract: true,
  });
  assert.equal(
    messageActionCommand("edit", other, "alice", [], elevatedOnly),
    "/sudo /edit",
  );
});

test("sudo commands wrap message actions when the direct route is missing", () => {
  assert.equal(
    messageActionCommand("edit", other, "alice", [], sudoCommands),
    "/sudo /edit",
  );
  assert.equal(
    messageActionCommand("retract", other, "alice", [], sudoCommands),
    "/sudo /retract",
  );
});

test("direct message grants keep using unwrapped commands", () => {
  const permissions = ["w:message.edit.any", "w:message.retract.any"];
  assert.equal(
    messageActionCommand("edit", other, "alice", permissions, sudoCommands),
    "/edit",
  );
  assert.equal(
    messageActionCommand("retract", other, "alice", permissions, sudoCommands),
    "/retract",
  );
});

test("sudo exposes only commands it actually advertises", () => {
  const limited = [
    ...commands,
    { name: "/sudo", subcommands: [{ name: "/system" }] },
  ];
  assert.deepEqual(messageActions(other, "alice", [], limited), {
    react: false,
    reply: false,
    edit: false,
    retract: false,
  });
  assert.equal(messageActionCommand("edit", other, "alice", [], limited), null);
  assert.equal(
    messageActionCommand(
      "edit",
      other,
      "alice",
      [],
      [...commands, { name: "/sudo" }],
    ),
    null,
  );
});
