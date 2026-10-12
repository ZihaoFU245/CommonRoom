import test from "node:test";
import assert from "node:assert/strict";
import {
  markdownNodes,
  loadMarkdown,
  setMarkdown,
} from "../src/features/conversation/markdown.ts";
import { submissionFrame } from "../src/features/conversation/submission.ts";
import { isMessage } from "../src/api/protocol.ts";
import {
  browserCommands,
  setAlias,
} from "../src/features/conversation/aliases.ts";

function flatten(nodes) {
  return nodes.flatMap((node) =>
    typeof node === "string" ? [node] : [node, ...flatten(node.children)],
  );
}
test("Markdown supports blocks, nested inline formatting and literal code", () => {
  const nodes = flatten(
    markdownNodes(
      "# Hello\n\n**bold and *italic*** ~~gone~~\n\n- one\n- two\n\n> quote\n\n```js\n<script>literal</script>\n```\n\n`**literal**`",
    ),
  );
  for (const tag of [
    "h1",
    "strong",
    "em",
    "s",
    "ul",
    "li",
    "blockquote",
    "pre",
    "code",
  ])
    assert.ok(
      nodes.some((node) => node.tag === tag),
      tag,
    );
  assert.ok(nodes.includes("<script>literal</script>\n"));
  assert.ok(nodes.includes("**literal**"));
});
test("HTML is text, unsafe links are inert, and images cannot load", () => {
  const nodes = flatten(
    markdownNodes(
      "<img src=x onerror=alert(1)> [bad](javascript:alert(1)) [data](data:text/html,test) [relative](/api/logout) ![tracking](https://evil.test/pixel) [safe](https://example.com)",
    ),
  );
  assert.ok(
    nodes.some((node) => typeof node === "string" && node.includes("<img")),
  );
  assert.ok(!nodes.some((node) => node.tag === "img"));
  assert.deepEqual(
    nodes.filter((node) => node.href).map((node) => node.href),
    ["https://example.com"],
  );
});
test("Markdown preference defaults off, persists and tolerates unavailable storage", () => {
  const storage = new Map();
  globalThis.localStorage = {
    getItem: (key) => storage.get(key),
    setItem: (key, value) => storage.set(key, value),
  };
  assert.equal(loadMarkdown(), false);
  assert.equal(setMarkdown("/markdown on"), true);
  assert.equal(loadMarkdown(), true);
  assert.equal(setMarkdown("/markdown off"), false);
  assert.equal(loadMarkdown(), false);
  for (const input of ["/markdown", "/markdown yes", "/markdown on extra"])
    assert.throws(() => setMarkdown(input), /Usage/);
  assert.throws(() => setAlias({}, "markdown", "ls"));
  assert.ok(
    browserCommands([], {}).some((command) => command.name === "/markdown"),
  );
  globalThis.localStorage = {
    getItem() {
      throw Error("Unavailable");
    },
    setItem() {
      throw Error("Unavailable");
    },
  };
  assert.equal(loadMarkdown(), false);
  assert.equal(setMarkdown("/markdown on"), true);
});
test("format is captured for room, private, explicit tell and reply submissions", () => {
  const base = {
    text: "**hello**",
    draft: "**hello**",
    selected: "lobby",
    consoleView: false,
    peer: null,
    replyId: null,
    markdown: true,
  };
  for (const overrides of [
    {},
    { peer: "bob", selected: "@direct:bob" },
    { replyId: "m1" },
    { text: "/tell bob **hello**" },
  ]) {
    assert.equal(submissionFrame({ ...base, ...overrides }).markdown, true);
  }
  assert.equal(submissionFrame({ ...base, markdown: false }).markdown, false);
  const message = {
    id: "m1",
    from: "bob",
    to: null,
    text: "hi",
    time: 1,
    sequence: 1,
    reactions: {},
    reply: null,
    mentions: [],
  };
  assert.ok(isMessage(message));
  assert.ok(isMessage({ ...message, markdown: true }));
  assert.equal(isMessage({ ...message, markdown: "on" }), false);
});
