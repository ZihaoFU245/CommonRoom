import test from "node:test";
import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { readFileSync } from "node:fs";
import ts from "typescript";
let index = 0;
const slots = [];
globalThis.avatarTestState = (value) => {
  const slot = index++;
  if (!(slot in slots)) slots[slot] = value;
  return [
    slots[slot],
    (next) => {
      slots[slot] = next;
    },
  ];
};
const hooks = registerHooks({
  resolve(specifier, context, next) {
    if (specifier.endsWith("/useExternalAvatars.ts"))
      return { url: "test:external-avatars", shortCircuit: true };
    return specifier === "preact/hooks"
      ? { url: "test:avatar-hooks", shortCircuit: true }
      : next(specifier, context);
  },
  load(url, context, next) {
    if (url === "test:external-avatars")
      return {
        format: "module",
        shortCircuit: true,
        source:
          "export const useExternalAvatars=()=>[globalThis.externalAllowed ?? false, ()=>{}];",
      };
    if (url === "test:avatar-hooks")
      return {
        format: "module",
        shortCircuit: true,
        source:
          "export const useState=(value)=>globalThis.avatarTestState(value);",
      };
    if (url.endsWith("/components/Avatar.tsx"))
      return {
        format: "module",
        shortCircuit: true,
        source: ts.transpileModule(readFileSync(new URL(url), "utf8"), {
          compilerOptions: {
            module: ts.ModuleKind.ESNext,
            jsx: ts.JsxEmit.ReactJSX,
            jsxImportSource: "preact",
          },
        }).outputText,
      };
    return next(url, context);
  },
});
const { Avatar } = await import("../src/components/Avatar.tsx");
hooks.deregister();
function render(url, fallback = false, props = {}) {
  index = 0;
  return Avatar({ url, name: "Alice", fallback, ...props });
}
function images(node) {
  return [node?.props?.children]
    .flat(Infinity)
    .filter((child) => child?.type === "img");
}
test("loaded avatars stay painted during replacement and new identities never inherit an old frame", (t) => {
  const original = globalThis.document;
  globalThis.document = { baseURI: "https://chat.example/team/" };
  globalThis.externalAllowed = true;
  t.after(() => {
    globalThis.document = original;
    delete globalThis.avatarTestState;
    delete globalThis.externalAllowed;
  });
  assert.equal(render(undefined), null);
  const source = "https://images.example/a.png";
  const pending = images(render(source))[0];
  assert.equal(pending.props.hidden, true);
  assert.equal(pending.props.width, 28);
  assert.equal(pending.props.height, 28);
  assert.equal(pending.props.alt, "");
  assert.equal(pending.props.referrerPolicy, "no-referrer");
  pending.props.onLoad();
  assert.equal(images(render(source))[0].props.hidden, false);
  const replacement = "https://images.example/b.png";
  let [painted, next] = images(render(replacement));
  assert.equal(painted.props.src, source);
  assert.equal(painted.props.hidden, false);
  assert.equal(next.props.hidden, true);
  next.props.onLoad();
  [painted] = images(render(replacement));
  assert.equal(painted.props.src, replacement);
  assert.equal(painted.props.hidden, false);
  const reused = images(render(replacement, false, { name: "Bob" }));
  assert.equal(reused.length, 1);
  assert.equal(reused[0].props.hidden, true);
  assert.equal(render(undefined), null);

  globalThis.externalAllowed = false;
  assert.equal(
    render(replacement),
    null,
    "external avatars disappear immediately when disabled",
  );
  const managed = "/api/avatars/12345678-1234-1234-1234-123456789abc/file";
  const managedImages = images(render(managed));
  assert.equal(
    managedImages.length,
    1,
    "disabled external frame cannot remain behind a managed replacement",
  );
  managedImages[0].props.onLoad();
  assert.equal(images(render(managed))[0].props.hidden, false);
  const local = images(
    render(managed, true, {
      preview: "blob:https://chat.example/local",
      large: true,
      decorative: false,
    }),
  );
  assert.equal(local[1].props.src, "blob:https://chat.example/local");
  assert.equal(local[1].props.width, 72);
  assert.equal(local[1].props.alt, "Alice's avatar");
  slots.length = 0;
  const failing = images(render(managed))[0];
  failing.props.onError();
  assert.equal(render(managed), null);
  assert.equal(render(managed, true).type, "span");
});
