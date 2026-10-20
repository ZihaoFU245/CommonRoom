import test from "node:test";
import assert from "node:assert/strict";
import {
  loadAliases,
  saveAlias,
  expandAlias,
  browserCommands,
  aliasHelp,
  aliasManual,
  setAlias,
  removeAlias,
} from "../src/features/conversation/aliases.ts";
import {
  redactCommand,
  suggestions,
} from "../src/features/conversation/console.ts";
import { commandNodes } from "../src/features/conversation/commandMarkdown.ts";

test("browser help and hints describe saved aliases without exposing passwords", () => {
  const aliases = {
    "/ls": "/room ls",
    "/make": "/user new bob secret",
  };
  const serverCommands = [{ name: "/ls", usage: "/ls", description: "List" }];
  const help = aliasHelp(aliases);
  assert.match(help, /\/man alias/);
  assert.ok(help.includes("- `/ls` — Alias for `/room ls`"));
  assert.ok(!help.includes("secret"));
  const hints = browserCommands(serverCommands, aliases);
  assert.equal(hints.filter((command) => command.name === "/ls").length, 1);
  assert.ok(!JSON.stringify(hints).includes("secret"));
  assert.equal(serverCommands.length, 1);
  assert.ok(
    commandNodes(aliasManual).some(
      (block) =>
        typeof block !== "string" &&
        block.tag === "pre" &&
        JSON.stringify(block.children).includes("/alias drop room delete"),
    ),
  );
  assert.equal(
    suggestions(
      "/man a",
      [{ name: "/man", usage: "/man [topic]", description: "Manual" }],
      [],
      [],
    )[0].value,
    "/man alias ",
  );
});

test("browser aliases persist and expand one command prefix, including sudo", () => {
  const stored = new Map();
  globalThis.localStorage = {
    getItem: (key) => stored.get(key) ?? null,
    setItem: (key, value) => stored.set(key, value),
  };
  const aliases = saveAlias({}, "/alias rooms /room ls");
  assert.deepEqual(loadAliases(), aliases);
  assert.equal(expandAlias("/rooms", aliases), "/room ls");
  const removal = saveAlias(aliases, "/alias drop user delete");
  assert.equal(
    expandAlias("/sudo /drop bob", removal),
    "/sudo /user delete bob",
  );
  assert.equal(expandAlias("hello /drop", removal), "hello /drop");
  const sameName = saveAlias(removal, "/alias ls room ls");
  assert.equal(expandAlias("/ls", sameName), "/room ls");
  assert.equal(expandAlias("/a", { "/a": "/b", "/b": "/a" }), "/b");
  assert.throws(() => saveAlias({}, "/alias / ls"));
  assert.throws(() => saveAlias({}, "/alias sudo ls"));
  assert.throws(() => saveAlias({}, "/alias alias ls"));
  assert.equal(
    redactCommand("/alias make /user new alice secret admin"),
    "/alias make /user new alice •••• admin",
  );
  assert.equal(
    redactCommand(expandAlias("/make alice secret", { "/make": "/user new" })),
    "/user new alice ••••",
  );
});
test("malformed and unavailable browser storage is handled", () => {
  globalThis.localStorage = {
    getItem: () => '{"/good":"/ls","bad":42}',
    setItem: () => {
      throw new Error("Unavailable");
    },
  };
  assert.deepEqual(loadAliases(), { "/good": "/ls" });
  assert.throws(() => saveAlias({}, "/alias rooms room ls"), /Unavailable/);
  globalThis.localStorage = {
    getItem: () => {
      throw new Error("Unavailable");
    },
  };
  assert.deepEqual(loadAliases(), {});
});

test("settings and slash commands share alias updates and persistent removal", () => {
  const stored = new Map();
  globalThis.localStorage = {
    getItem: (key) => stored.get(key) ?? null,
    setItem: (key, value) => stored.set(key, value),
  };
  const aliases = setAlias({}, " rooms ", " room ls ");
  assert.deepEqual(loadAliases(), { "/rooms": "/room ls" });
  const changed = saveAlias(aliases, "/alias rooms user ls");
  assert.equal(expandAlias("/rooms", changed), "/user ls");
  const removed = removeAlias(changed, "/rooms");
  assert.deepEqual(loadAliases(), {});
  assert.equal(expandAlias("/rooms", removed), "/rooms");
  assert.deepEqual(changed, { "/rooms": "/user ls" });
  for (const name of ["", "/", "two names", "/sudo", "/alias"])
    assert.throws(() => setAlias({}, name, "room ls"));
  assert.throws(() => setAlias({}, "rooms", ""));
  globalThis.localStorage.setItem = () => {
    throw new Error("Unavailable");
  };
  assert.throws(() => removeAlias(changed, "/rooms"), /Unavailable/);
  assert.deepEqual(changed, { "/rooms": "/user ls" });
});

test("alias targets retain canonical argument suggestions and password masking", () => {
  const aliases = { "/drop": "/user delete", "/ls": "/room ls" };
  const commands = [
    { name: "/user", usage: "/user delete name", description: "Remove" },
    { name: "/ls", usage: "/ls [room|user]", description: "List" },
  ];
  assert.equal(
    suggestions(
      expandAlias("/drop b", aliases),
      commands,
      ["bob"],
      ["board"],
    )[0].value,
    "/user delete bob ",
  );
  assert.equal(
    expandAlias("/sudo /drop bob", aliases),
    "/sudo /user delete bob",
  );
  assert.equal(
    redactCommand("/sudo /alias make user new alice secret admin"),
    "/sudo /alias make /user new alice •••• admin",
  );
});
