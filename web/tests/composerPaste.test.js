import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";
import { readFileSync } from "node:fs";
import ts from "typescript";

const hooks = registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "preact/hooks")
      return { url: "test:composer-hooks", shortCircuit: true };
    if (specifier.endsWith("/useCompositionGuard.ts"))
      return { url: "test:composition-guard", shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url === "test:composer-hooks")
      return {
        format: "module",
        shortCircuit: true,
        source:
          "export const useRef=(value)=>({current:value});export const useEffect=()=>{};",
      };
    if (url === "test:composition-guard")
      return {
        format: "module",
        shortCircuit: true,
        source: "export const useCompositionGuard=()=>({});",
      };
    if (url.endsWith("/components/Composer.tsx"))
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
const { Composer } = await import("../src/components/Composer.tsx");
hooks.deregister();

// The resize separator reads the viewport height while rendering.
globalThis.window = { innerHeight: 800 };

function props(overrides = {}) {
  return {
    mediaUpload: {
      enabled: true,
      maxSize: 1000,
      attachment: null,
      busy: false,
      status: "",
      error: "",
      start: async () => {},
      cancel: () => {},
      retry: undefined,
    },
    minimumComposerHeight: 42,
    actualComposerHeight: 42,
    hints: [],
    activeHint: 0,
    replyTarget: null,
    input: { current: null },
    consoleView: false,
    draft: "",
    pending: false,
    canWrite: true,
    startResize: () => {},
    moveResize: () => {},
    endResize: () => {},
    resizeComposer: () => {},
    acceptHint: () => {},
    cancelReply: () => {},
    send: () => {},
    edit: () => {},
    keydown: () => {},
    selectCaret: () => {},
    ...overrides,
  };
}

function walk(node, visit) {
  if (Array.isArray(node)) {
    for (const child of node) walk(child, visit);
    return;
  }
  if (!node || typeof node !== "object" || typeof node.type !== "string")
    return;
  visit(node);
  walk(node.props?.children, visit);
}

function element(type, overrides = {}) {
  let found = null;
  walk(Composer(props(overrides)), (node) => {
    if (node.type === type) found = node;
  });
  return found;
}

function recording(mediaOverrides = {}, overrides = {}) {
  const started = [];
  const mediaUpload = {
    ...props().mediaUpload,
    ...mediaOverrides,
    start: async (file) => {
      started.push(file);
    },
  };
  return { started, overrides: { ...overrides, mediaUpload } };
}

function imageClipboard(name = "image.png") {
  const file = new File([new Uint8Array([137, 80, 78, 71])], name, {
    type: "image/png",
  });
  return {
    file,
    data: { items: [{ kind: "file", type: file.type, getAsFile: () => file }] },
  };
}

function paste(clipboardData, overrides = {}) {
  let prevented = 0;
  element("textarea", overrides).props.onPaste({
    clipboardData,
    preventDefault: () => {
      prevented += 1;
    },
  });
  return prevented;
}

test("pasting a clipboard image starts the same upload the file picker starts", () => {
  const { started, overrides } = recording();
  const picker = element("input", overrides);
  assert.equal(picker.props.disabled, false, "the attach picker is available");

  const { data, file } = imageClipboard();
  assert.equal(
    paste(data, overrides),
    1,
    "the image never reaches the textarea",
  );
  assert.equal(started.length, 1);
  assert.equal(started[0], file);

  const picked = new File(["picked"], "picked.png", { type: "image/png" });
  picker.props.onChange({ currentTarget: { files: [picked], value: "" } });
  assert.deepEqual(started, [file, picked], "paste and picker share one path");
});

test("unnamed clipboard images are uploaded under a generated filename", () => {
  const { started, overrides } = recording();
  const file = new File([new Uint8Array([1])], "", { type: "image/png" });
  const data = {
    items: [{ kind: "file", type: file.type, getAsFile: () => file }],
  };
  assert.equal(paste(data, overrides), 1);
  assert.equal(started.length, 1);
  assert.equal(started[0].name, "pasted-image.png");
  assert.equal(started[0].type, "image/png");
});

test("text and non-image pastes keep the browser's own behavior", () => {
  const { started, overrides } = recording();
  assert.equal(
    paste(
      {
        items: [{ kind: "string", type: "text/plain", getAsFile: () => null }],
        files: [],
      },
      overrides,
    ),
    0,
  );
  assert.equal(
    paste(
      {
        items: [],
        files: [new File(["pdf"], "a.pdf", { type: "application/pdf" })],
      },
      overrides,
    ),
    0,
  );
  assert.deepEqual(started, []);
});

test("paste is refused exactly where the attach picker is disabled", () => {
  const cases = [
    ["console view", {}, { consoleView: true }],
    ["read-only conversation", {}, { canWrite: false }],
    ["while sending", {}, { pending: true }],
    ["while uploading", { busy: true }, {}],
    [
      "with an attachment",
      {
        attachment: {
          media: { filename: "image.png", size: 4 },
          preview: null,
        },
      },
      {},
    ],
    ["while writing a command", {}, { draft: "/help" }],
    ["without media support", { enabled: false }, {}],
  ];
  for (const [label, media, overrides] of cases) {
    const recorded = recording(media, overrides);
    assert.equal(
      paste(imageClipboard().data, recorded.overrides),
      0,
      `${label}: the paste falls through`,
    );
    assert.deepEqual(recorded.started, [], `${label}: no upload starts`);
    const merged = props(recorded.overrides);
    const picker = element("input", recorded.overrides);
    if (merged.consoleView || !merged.canWrite || !merged.mediaUpload.enabled)
      assert.equal(picker, null, `${label}: the picker is hidden`);
    else
      assert.equal(
        picker.props.disabled,
        true,
        `${label}: the picker is disabled`,
      );
  }
});
