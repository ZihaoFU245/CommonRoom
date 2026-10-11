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
import { manualBlocks } from "../src/features/conversation/manual.ts";

test("browser help and hints describe saved aliases without exposing passwords", () => {
  const aliases = {
    "/ls": "/ls room",
    "/make": "/new user bob secret",
  };
  const serverCommands = [{ name: "/ls", usage: "/ls", description: "List" }];
  const help = aliasHelp(aliases);
  assert.match(help, /\/man alias/);
  assert.match(help, /\/ls — Alias for \/ls room/);
  assert.ok(!help.includes("secret"));
  const hints = browserCommands(serverCommands, aliases);
  assert.equal(hints.filter((command) => command.name === "/ls").length, 1);
  assert.ok(!JSON.stringify(hints).includes("secret"));
  assert.equal(serverCommands.length, 1);
  assert.ok(
    manualBlocks(aliasManual).some(
      (block) =>
        block.kind === "code" && block.text.includes("/alias drop rm room"),
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
  const aliases = saveAlias({}, "/alias rooms /ls room");
  assert.deepEqual(loadAliases(), aliases);
  assert.equal(expandAlias("/rooms", aliases), "/ls room");
  const removal = saveAlias(aliases, "/alias drop rm user");
  assert.equal(expandAlias("/sudo /drop bob", removal), "/sudo /rm user bob");
  assert.equal(expandAlias("hello /drop", removal), "hello /drop");
  const sameName = saveAlias(removal, "/alias ls ls room");
  assert.equal(expandAlias("/ls", sameName), "/ls room");
  assert.equal(expandAlias("/a", { "/a": "/b", "/b": "/a" }), "/b");
  assert.throws(() => saveAlias({}, "/alias / ls"));
  assert.throws(() => saveAlias({}, "/alias sudo ls"));
  assert.throws(() => saveAlias({}, "/alias alias ls"));
  assert.equal(
    redactCommand("/alias make /new user alice secret admin"),
    "/alias make /new user alice •••• admin",
  );
  assert.equal(
    redactCommand(expandAlias("/make alice secret", { "/make": "/new user" })),
    "/new user alice ••••",
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
  assert.throws(() => saveAlias({}, "/alias rooms ls room"), /Unavailable/);
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
  const aliases = setAlias({}, " rooms ", " ls room ");
  assert.deepEqual(loadAliases(), { "/rooms": "/ls room" });
  const changed = saveAlias(aliases, "/alias rooms ls user");
  assert.equal(expandAlias("/rooms", changed), "/ls user");
  const removed = removeAlias(changed, "/rooms");
  assert.deepEqual(loadAliases(), {});
  assert.equal(expandAlias("/rooms", removed), "/rooms");
  assert.deepEqual(changed, { "/rooms": "/ls user" });
  for (const name of ["", "/", "two names", "/sudo", "/alias"])
    assert.throws(() => setAlias({}, name, "ls room"));
  assert.throws(() => setAlias({}, "rooms", ""));
  globalThis.localStorage.setItem = () => {
    throw new Error("Unavailable");
  };
  assert.throws(() => removeAlias(changed, "/rooms"), /Unavailable/);
  assert.deepEqual(changed, { "/rooms": "/ls user" });
});

test("alias targets retain canonical argument suggestions and password masking", () => {
  const aliases = { "/drop": "/rm user", "/ls": "/ls room" };
  const commands = [
    { name: "/rm", usage: "/rm user name", description: "Remove" },
    { name: "/ls", usage: "/ls [room|user]", description: "List" },
  ];
  assert.equal(
    suggestions(
      expandAlias("/drop b", aliases),
      commands,
      ["bob"],
      ["board"],
    )[0].value,
    "/rm user bob ",
  );
  assert.equal(expandAlias("/sudo /drop bob", aliases), "/sudo /rm user bob");
  assert.equal(
    redactCommand("/sudo /alias make new user alice secret admin"),
    "/sudo /alias make /new user alice •••• admin",
  );
});
