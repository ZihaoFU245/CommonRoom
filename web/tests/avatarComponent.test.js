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
    return specifier === "preact/hooks"
      ? { url: "test:avatar-hooks", shortCircuit: true }
      : next(specifier, context);
  },
  load(url, context, next) {
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
function render(url, fallback = false) {
  index = 0;
  return Avatar({ url, name: "Alice", fallback });
}
function img(node) {
  return node?.props?.children?.find((child) => child?.type === "img");
}
test("avatars reserve no image space before load or after failure and recover on URL changes", (t) => {
  const original = globalThis.document;
  globalThis.document = { baseURI: "https://chat.example/team/" };
  t.after(() => {
    globalThis.document = original;
    delete globalThis.avatarTestState;
  });
  assert.equal(render(undefined), null);
  const source = "https://images.example/a.png";
  const pending = img(render(source));
  assert.equal(pending.props.hidden, true);
  assert.equal(pending.props.referrerPolicy, "no-referrer");
  pending.props.onLoad();
  assert.equal(img(render(source)).props.hidden, false);
  const replacement = "https://images.example/b.png";
  assert.equal(
    img(render(replacement)).props.hidden,
    true,
    "old loaded URL cannot reveal new unready image",
  );
  img(render(replacement)).props.onError();
  assert.equal(
    render(replacement),
    null,
    "failed images leave no wrapper or broken image",
  );
  assert.equal(
    render(replacement, true).type,
    "span",
    "Account retains an add-avatar target",
  );
  const next = "https://images.example/c.png";
  img(render(next)).props.onLoad();
  assert.equal(img(render(next)).props.hidden, false);
  assert.equal(render(undefined), null, "removing avatar removes its element");
});
