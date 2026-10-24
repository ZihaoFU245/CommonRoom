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
  "/user",
  "/room",
  "/history",
  "/reply",
  "/react",
  "/retract",
  "/alias",
  "/tell",
  "/grant",
  "/sudo",
  "/clean",
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
  assert.deepEqual(values("/bot config "), []);
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
    assert.deepEqual(values(prefix), [prefix + "#team ", prefix + "@global "]);
  }
  assert.deepEqual(values("/bot owner alice b"), ["/bot owner alice bob "]);
  assert.deepEqual(values("/bot rm bob "), ["/bot rm bob @global "]);
  for (const draft of [
    "/bot new w",
    "/bot token new bob label",
    "/bot token rm bob id",
    "/bot config set bob key",
    "/user passwd secret ",
  ]) {
    assert.deepEqual(values(draft), []);
  }
});
test("remaining command arguments complete known values and wrapped commands", () => {
  assert.deepEqual(values("/debug "), ["/debug on ", "/debug off "]);
  assert.deepEqual(values("/room owner b"), ["/room owner bob "]);
  assert.deepEqual(values("/room owner bob #t"), ["/room owner bob #team "]);
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
  assert.deepEqual(values("/grant bob #team w:bot"), [
    "/grant bob #team w:bot.config ",
  ]);
  assert.deepEqual(values("/alias /hi /bot config f"), []);
  assert.deepEqual(values("/sudo /bot config f"), []);
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

test("object commands complete scopes, metadata, bulk users and rename targets", () => {
  assert.deepEqual(values("/user ls #"), ["/user ls #team "]);
  assert.deepEqual(values("/user get @"), ["/user get @all "]);
  assert.deepEqual(values("/user get @all #"), ["/user get @all #team "]);
  assert.deepEqual(values("/user set @all media.max_"), [
    "/user set @all media.max_size ",
    "/user set @all media.max_age ",
  ]);
  assert.deepEqual(values("/user set @all media.max_age 7d #"), [
    "/user set @all media.max_age 7d #team ",
  ]);
  assert.deepEqual(values("/user new created secret a"), [
    "/user new created secret admin ",
  ]);
  assert.deepEqual(values("/room rename #t"), ["/room rename #team "]);
  assert.deepEqual(values("/room rename #team renamed"), []);
  assert.deepEqual(values("/user rename b"), ["/user rename bob "]);
  assert.deepEqual(values("/user rename bob renamed"), []);
  assert.deepEqual(values("/grant @"), ["/grant @all "]);
  assert.deepEqual(values("/sudo /room delete #t"), [
    "/sudo /room delete #team ",
  ]);
});

test("completion shows only the current command level and stops at free-form text", () => {
  const family = {
    name: "/user",
    usage:
      "/user ls [scope] | get who [key] [scope] | set who key value [scope]",
    description: "Manage users",
  };
  const root = suggestions("/u", [family], ["bob"], ["team"]);
  assert.deepEqual(
    root.map((hint) => hint.label),
    ["/user"],
  );
  assert.equal(root[0].advance, true);
  const children = suggestions(root[0].value, [family], ["bob"], ["team"]);
  assert.ok(children.some((hint) => hint.label === "set"));
  for (const hint of children) {
    assert.ok(!hint.label.includes(" "));
    assert.ok(!hint.description.includes(" | "));
    assert.equal(hint.advance, true);
  }
  assert.deepEqual(
    suggestions("/user set bob label freeform", [family], ["bob"], []),
    [],
  );
  assert.deepEqual(values("/grant bob #team /user "), []);
});

test("all explicit room scope suggestions use # and metadata get offers @all keys", () => {
  for (const draft of [
    "/grant bob user ",
    "/permissions ",
    "/bot ls ",
    "/user ls ",
    "/clean 7d ",
  ]) {
    const hints = suggestions(draft, commands, ["bob"], ["team"], context);
    assert.ok(
      hints.some((hint) => hint.value.endsWith("#team ")),
      draft,
    );
    assert.ok(!hints.some((hint) => hint.value.endsWith(" team ")), draft);
  }
  assert.ok(values("/user get bob @").includes("/user get bob @all "));
  assert.ok(!values("/user set bob @").includes("/user set bob @all "));
});

test("cleanup suggests explicit private targets and a preview flag after the scope", () => {
  const hints = suggestions(
    "/clean 7d ",
    commands,
    ["alice", "bob"],
    ["team"],
    {
      privateTargets: ["@direct:bob", "@private:alice:carol"],
    },
  );
  assert.deepEqual(
    hints.map((hint) => hint.label),
    ["#team", "@global", "@private", "@direct:bob", "@private:alice:carol"],
  );
  assert.equal(
    suggestions("/clean 7d @direct:bob --d", commands, [], [])[0].value,
    "/clean 7d @direct:bob --dry-run ",
  );
  assert.deepEqual(
    suggestions("/clean 7d @direct:bob --dry-run ", commands, [], []),
    [],
  );
});

test("cleanup metadata and all-history syntax have concise completions", () => {
  assert.deepEqual(values("/user set bob history."), [
    "/user set bob history.clean_min_age ",
  ]);
  assert.deepEqual(values("/user set bob history.clean_min_age -"), [
    "/user set bob history.clean_min_age -1 ",
  ]);
  assert.deepEqual(values("/clean -"), ["/clean -1 "]);
  assert.deepEqual(values("/grant bob #team x:history.clean "), []);
});

test("user reset completes metadata, scopes and the explicit password selector", () => {
  assert.deepEqual(values("/user reset @"), ["/user reset @all "]);
  assert.deepEqual(values("/user reset bob p"), ["/user reset bob password "]);
  assert.deepEqual(values("/user reset bob @"), ["/user reset bob @all "]);
  assert.deepEqual(values("/user reset bob media.max_age #"), [
    "/user reset bob media.max_age #team ",
  ]);
  assert.deepEqual(values("/user reset bob password "), []);
  assert.equal(
    values("/user reset @all ").includes("/user reset @all password "),
    false,
  );
});

test("user metadata completes color keys, palette values, resets and scopes", () => {
  for (const operation of ["set", "get", "reset"]) {
    assert.deepEqual(values(`/user ${operation} bob col`), [
      `/user ${operation} bob color `,
    ]);
  }
  const palette = [
    "red",
    "orange",
    "amber",
    "yellow",
    "lime",
    "green",
    "emerald",
    "teal",
    "cyan",
    "sky",
    "blue",
    "indigo",
    "violet",
    "purple",
    "fuchsia",
    "pink",
    "@unset",
  ];
  assert.deepEqual(
    values("/user set bob color "),
    palette.map((color) => `/user set bob color ${color} `),
  );
  assert.deepEqual(values("/user set @all color bl"), [
    "/user set @all color blue ",
  ]);
  assert.deepEqual(values("/user set bob color @"), [
    "/user set bob color @unset ",
  ]);
  assert.deepEqual(values("/user set bob color blue "), [
    "/user set bob color blue @global ",
    "/user set bob color blue #team ",
  ]);
});
