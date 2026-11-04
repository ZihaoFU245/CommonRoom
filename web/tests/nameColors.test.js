import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  nameColorOf,
  resolvedColors,
  mergeMessageColors,
  nameColors,
  sameColors,
  scopedColors,
} from "../src/features/preferences/nameColors.ts";

test("room overrides replace global colors only for the names they cover", () => {
  const global = { alice: "red", bob: "blue" };
  assert.equal(scopedColors(global, undefined), global);
  assert.deepEqual(scopedColors(undefined, undefined), {});
  assert.deepEqual(scopedColors(global, { bob: "teal", eve: "pink" }), {
    alice: "red",
    bob: "teal",
    eve: "pink",
  });
  assert.deepEqual(global, { alice: "red", bob: "blue" });
});

test("name colors fall back to the default for unknown or missing entries", () => {
  const colors = { alice: "red", bob: "#ff0000", eve: "white" };
  assert.equal(nameColorOf(colors, "alice"), "red");
  assert.equal(nameColorOf(colors, "bob"), undefined);
  assert.equal(nameColorOf(colors, "eve"), undefined);
  assert.equal(nameColorOf(colors, "missing"), undefined);
  assert.equal(nameColorOf(colors, "constructor"), undefined);
});

test("every palette name has a light and dark shade and matches the server", () => {
  const css = ["base", "name-colors"]
    .map((name) =>
      readFileSync(
        new URL(`../src/styles/${name}.css`, import.meta.url),
        "utf8",
      ),
    )
    .join("\n");
  const rust = readFileSync(
    new URL("../../server/src/engine/colors.rs", import.meta.url),
    "utf8",
  );
  const server = [
    .../NAME_COLORS[^=]*=\s*\[([^\]]*)\]/u.exec(rust)[1].matchAll(/"(\w+)"/gu),
  ].map((match) => match[1]);
  assert.deepEqual(server, [...nameColors]);
  for (const color of nameColors) {
    assert.equal(css.split(`--name-${color}:`).length, 3, color);
    assert.ok(css.includes(`[data-name-color="${color}"]`), color);
  }
});

test("unchanged color maps compare equal regardless of identity", () => {
  assert.ok(sameColors({ a: "red" }, { a: "red" }));
  assert.ok(sameColors({}, {}));
  assert.ok(!sameColors({ a: "red" }, { a: "blue" }));
  assert.ok(!sameColors({ a: "red" }, { a: "red", b: "red" }));
  assert.ok(!sameColors({ a: "red", b: "red" }, { a: "red" }));
});

test("resolved page colors override inherited defaults and expire on metadata changes", () => {
  const base = { bob: "blue", eve: "pink" };
  const page = { version: "instance:1", values: { bob: "red", eve: null } };
  assert.deepEqual(resolvedColors(base, "instance:1", page), { bob: "red" });
  assert.deepEqual(resolvedColors(base, "instance:2", page), base);
  assert.deepEqual(resolvedColors(base, "restart:1", page), base);
  assert.deepEqual(base, { bob: "blue", eve: "pink" });
});

test("merging page colors retains disclosed names and discards old versions and evicted names", () => {
  const messages = [
    { from: "bob", mentions: ["eve"], reply: { from: "alice" } },
  ];
  const old = {
    version: "one",
    values: { bob: "red", alice: "teal", hidden: "pink" },
  };
  const next = { version: "one", values: { eve: "blue", bob: null } };
  assert.deepEqual(mergeMessageColors(old, next, messages), {
    version: "one",
    values: { bob: null, alice: "teal", eve: "blue" },
  });
  assert.deepEqual(
    mergeMessageColors(old, { ...next, version: "two" }, messages),
    {
      version: "two",
      values: { eve: "blue", bob: null },
    },
  );
  const unusual = JSON.parse('{"__proto__":"red","constructor":"teal"}');
  const resolved = resolvedColors({}, "one", {
    version: "one",
    values: unusual,
  });
  assert.equal(nameColorOf(resolved, "__proto__"), "red");
  assert.equal(nameColorOf(resolved, "constructor"), "teal");
});
