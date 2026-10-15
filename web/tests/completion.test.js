import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { suggestions } from "../src/features/conversation/console.ts";
import {
  cursorSuggestions,
  nextHint,
  permissionNames,
} from "../src/features/conversation/completion.ts";

const names = [
  "/bot",
  "/debug",
  "/permissions",
  "/owner",
  "/history",
  "/reply",
  "/react",
  "/retract",
  "/alias",
  "/tell",
  "/grant",
  "/sudo",
  "/passwd",
];
const commands = names.map((name) => ({
  name,
  usage: `${name} arguments`,
  description: "Command",
}));
const context = {
  aliases: ["/hi"],
  privateScopes: ["@private:alice:bob"],
  messages: [{ id: "message-1", reactions: { "🙂": ["bob"] } }],
};
function values(draft) {
  return suggestions(draft, commands, ["alice", "bob"], ["team"], context).map(
    (hint) => hint.value,
  );
}
test("command and argument matches are unlimited and navigation wraps in both directions", () => {
  assert.equal(values("/").length, names.length);
  for (let index = 0; index < names.length; index++) {
    assert.equal(nextHint(index, names.length), (index + 1) % names.length);
    assert.equal(
      nextHint(index, names.length, true),
      (index + names.length - 1) % names.length,
    );
  }
});
test("bot subcommands complete operations, account names and valid scopes", () => {
  assert.deepEqual(
    values("/bot "),
    ["new", "ls", "info", "owner", "rm", "token"].map(
      (value) => `/bot ${value} `,
    ),
  );
  assert.deepEqual(
    values("/bot token "),
    ["new", "ls", "rm"].map((value) => `/bot token ${value} `),
  );
  assert.deepEqual(values("/bot config "), [null]);
  for (const prefix of [
    "/bot info ",
    "/bot owner ",
    "/bot rm ",
    "/bot token new ",
    "/bot token ls ",
    "/bot token rm ",
  ]) {
    assert.deepEqual(values(prefix), [prefix + "alice ", prefix + "bob "]);
  }
  for (const prefix of ["/bot new worker ", "/bot ls ", "/bot info bob "]) {
    assert.deepEqual(values(prefix), [prefix + "team ", prefix + "@global "]);
  }
  assert.deepEqual(values("/bot owner alice b"), ["/bot owner alice bob "]);
  assert.deepEqual(values("/bot rm bob "), ["/bot rm bob @global "]);
  for (const draft of [
    "/bot new w",
    "/bot token new bob label",
    "/bot token rm bob id",
    "/bot config set bob key",
    "/passwd secret ",
  ]) {
    assert.deepEqual(values(draft), [null]);
  }
});
test("remaining command arguments complete known values and wrapped commands", () => {
  assert.deepEqual(values("/debug "), ["/debug on ", "/debug off "]);
  assert.deepEqual(values("/owner b"), ["/owner bob "]);
  assert.deepEqual(values("/owner bob t"), ["/owner bob team "]);
  assert.deepEqual(values("/history #"), ["/history #team "]);
  for (const name of ["/react", "/reply", "/retract"])
    assert.deepEqual(values(name + " m"), [name + " message-1 "]);
  assert.deepEqual(values("/react message-1 "), ["/react message-1 🙂 "]);
  assert.deepEqual(values("/permissions @account:b"), [
    "/permissions @account:bob ",
  ]);
  assert.deepEqual(values("/permissions @private:"), [
    "/permissions @private:alice:bob ",
  ]);
  assert.deepEqual(values("/grant bob team w:bot"), [
    "/grant bob team w:bot.config ",
  ]);
  assert.deepEqual(values("/alias /hi /bot config f"), [null]);
  assert.deepEqual(values("/sudo /bot config f"), [null]);
});
test("cursor completion replaces only the current token and preserves following Unicode text", () => {
  const draft = "/tell bo hello 🙂";
  const completion = cursorSuggestions(draft, 8, (prefix) =>
    suggestions(prefix, commands, ["bob"], []),
  );
  assert.equal(completion.hints[0].value, "/tell bob hello 🙂");
  assert.equal(completion.suffix, " hello 🙂");
  assert.equal(
    cursorSuggestions("/te", 3, (prefix) =>
      suggestions(prefix, commands, [], []),
    ).hints[0].value,
    "/tell ",
  );
});
test("permission completions match the server action registry", () => {
  const source = readFileSync(
    new URL("../../server/src/engine/authorization.rs", import.meta.url),
    "utf8",
  );
  const actions = [...source.matchAll(/=> "([rwx]:[a-z.]+)"/g)].map(
    (match) => match[1],
  );
  assert.deepEqual([...permissionNames].sort(), actions.sort());
});
