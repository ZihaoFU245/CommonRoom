import {
  loadMarkdown,
  setMarkdown,
} from "../src/features/preferences/markdown.ts";
import { Script } from "node:vm";
import { snakeSource } from "../scripts/markdown/snake.mjs";
import { safeMediaUrl } from "../src/features/conversation/mediaUrl.ts";
import {
  markdownNodes,
  htmlPreview,
} from "../src/features/conversation/richMarkdown.ts";
import { hasRawHtml } from "../src/features/conversation/markdown.ts";
import test from "node:test";
import assert from "node:assert/strict";
import { markdownNodes as plainNodes } from "../src/features/conversation/markdown.ts";
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
test("HTML is text, unsafe links are inert, and Markdown images render", () => {
  const nodes = flatten(
    markdownNodes(
      "<img src=x onerror=alert(1)> [bad](javascript:alert(1)) [data](data:text/html,test) [relative](/api/logout) ![tracking](https://evil.test/pixel) [safe](https://example.com)",
    ),
  );
  assert.ok(
    nodes.some((node) => typeof node === "string" && node.includes("<img")),
  );
  assert.equal(nodes.filter((node) => node.tag === "img").length, 1);
  assert.deepEqual(
    nodes.filter((node) => node.href).map((node) => node.href),
    ["https://example.com"],
  );
});

test("HTML formatting is opt-in and composes with Markdown without changing code", () => {
  const source =
    "<details><summary>More</summary><p>Hello <u>@alice</u> H<sub>2</sub>O x<sup>2</sup></p></details>\n\n**bold** <i>italic</i>\n\n`<b>literal</b>`\n\n```html\n<u>literal</u>\n```";
  const off = flatten(markdownNodes(source));
  assert.ok(!off.some((node) => node.tag === "details"));
  const on = flatten(markdownNodes(source, true));
  for (const tag of ["details", "summary", "u", "sub", "sup", "strong", "i"])
    assert.ok(
      on.some((node) => node.tag === tag),
      tag,
    );
  assert.ok(on.includes("@alice"));
  assert.ok(on.includes("<b>literal</b>"));
  assert.ok(on.includes("<u>literal</u>\n"));
  assert.deepEqual(markdownNodes(source, false), markdownNodes(source));
});
test("enabled HTML drops active content, arbitrary attributes and unsafe URLs", () => {
  const nodes = flatten(
    markdownNodes(
      '<div id="app" class="settings-dialog" style="position:fixed" onclick="alert(1)"><b onmouseover="alert(1)">safe</b><script>alert(1)</script><style>body{display:none}</style><iframe src="https://evil.test"></iframe><img src="https://evil.test/pixel" onerror="alert(1)"><svg><a href="https://evil.test">foreign</a></svg><math><mtext>foreign</mtext></math><form action="/api/logout"><input autofocus></form></div>\n\n<a href="jav&#x61;script:alert(1)">bad</a> <a href="data:text/html,evil">data</a> <a href="//evil.test">relative</a> <a href="/api/logout">local</a> <a href="https://example.com" target="_self" onclick="alert(1)">safe</a> <a href="mailto:a@example.com">mail</a>\n\n![tracking](https://evil.test/pixel)',
      true,
    ),
  );
  assert.deepEqual(
    nodes.filter((node) => node.href).map((node) => node.href),
    ["https://example.com", "mailto:a@example.com"],
  );
  for (const node of nodes.filter((node) => typeof node !== "string")) {
    assert.ok(
      ["div", "b", "p", "a", "span", "img"].includes(node.tag),
      node.tag,
    );
    assert.ok(
      Object.keys(node).every((key) =>
        ["tag", "children", "href", "image"].includes(key),
      ),
    );
  }
  assert.ok(!nodes.includes("alert(1)"));
  assert.ok(nodes.some((node) => node.image?.alt === "tracking"));
});
test("enabled HTML preserves ordered lists and repairs malformed nested markup safely", () => {
  const nodes = flatten(
    markdownNodes(
      '<ol start="3"><li><b>first</b></li><li>second</li></ol>\n\n<b><i>nested</b> text</i><img src=x>',
      true,
    ),
  );
  assert.ok(nodes.some((node) => node.tag === "ol" && node.start === "3"));
  assert.ok(nodes.some((node) => node.tag === "b"));
  assert.ok(nodes.some((node) => node.tag === "i"));
  assert.ok(!nodes.some((node) => node.tag === "img"));
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

test("raw HTML detection follows tokens, excluding code, autolinks and images", () => {
  for (const text of [
    "**hello**",
    "$x^2$",
    "![x](https://example.com/x)",
    "<https://example.com>",
    "<a@example.com>",
    "`<b>literal</b>`",
    "```html\n<b>literal</b>\n```",
    "1 < 2",
  ])
    assert.equal(hasRawHtml(text), false, text);
  for (const text of [
    "<b>hello</b>",
    "<!-- comment -->",
    "<div>block</div>",
    "text <img src=x>",
  ])
    assert.equal(hasRawHtml(text), true, text);
});

test("Markdown images validate URLs on both paths and raw media never renders", () => {
  const input =
    '<video src="https://example.com/movie.mp4"><source src="https://example.com/x"></video><audio src="https://example.com/a"></audio><picture><img src="https://example.com/raw"></picture>\n\n![**gif**](https://example.com/demo.gif)';
  for (const nodes of [plainNodes(input), markdownNodes(input, true)]) {
    const flat = flatten(nodes);
    assert.equal(flat.filter((node) => node.tag === "img").length, 1);
    assert.deepEqual(flat.find((node) => node.image).image, {
      src: "https://example.com/demo.gif",
      alt: "gif",
    });
    assert.ok(
      !flat.some((node) =>
        ["video", "audio", "source", "picture", "track"].includes(node.tag),
      ),
    );
  }
  for (const url of [
    "javascript:alert(1)",
    "vbscript:x",
    "file:///tmp/x",
    "data:text/html;base64,QQ==",
    "data:image/svg+xml;base64,QQ==",
    "/local.png",
    "//host/x",
    "https:",
    "https://",
    "https://example.com/ x",
    "data:image/png;base64,=",
    "data:image/png;base64,",
    "",
    "   ",
  ])
    assert.equal(safeMediaUrl(url), false, url);
  for (const url of [
    "http://example.com/x",
    "https://example.com/x.gif",
    "data:image/gif;base64,R0lGODlhAQABAAAAACw=",
  ])
    assert.equal(safeMediaUrl(url), true, url);
  for (const html of [false, true]) {
    for (const url of [
      "/local.png",
      "//host/x",
      "javascript:alert(1)",
      "data:text/html;base64,QQ==",
      "data:image/svg+xml;base64,QQ==",
    ]) {
      const nodes = flatten(markdownNodes(`![fallback](${url})`, html));
      assert.ok(!nodes.some((node) => node.image));
      assert.ok(nodes.includes("fallback"), url);
    }
  }
});

test("sanitizer unwraps benign unknown tags, drops dangerous subtrees, and rejects forged placeholders", () => {
  const nodes = flatten(
    markdownNodes(
      '<section><custom>keep <b>bold</b></custom></section><script><b>drop</b></script><template>hidden</template><span data-commonroom-generated="0">ordinary</span>\n\n![real](https://example.com/real.png)',
      true,
    ),
  );
  assert.ok(nodes.includes("keep "));
  assert.ok(nodes.includes("ordinary"));
  assert.ok(!nodes.includes("hidden"));
  assert.equal(nodes.filter((node) => node.image).length, 1);
});

test("snake game message keeps its board but cannot run scripts or controls", () => {
  assert.doesNotThrow(
    () => new Script(snakeSource.split("<script>")[1].split("</script>")[0]),
  );
  assert.equal(hasRawHtml(snakeSource), true);
  const text = flatten(plainNodes(snakeSource));
  assert.ok(
    text.some((node) => typeof node === "string" && node.includes("<script>")),
  );
  const rendered = flatten(markdownNodes(snakeSource, true));
  assert.ok(rendered.some((node) => node.tag === "pre"));
  assert.ok(
    rendered.some((node) => typeof node === "string" && node.includes("🐍")),
  );
  assert.ok(
    !rendered.some((node) =>
      ["script", "style", "button", "canvas"].includes(node.tag),
    ),
  );
  assert.ok(!JSON.stringify(rendered).includes("snakeExecuted"));
  assert.ok(!JSON.stringify(rendered).includes("onclick"));
});

test("HTML preview classifies active content and keeps full documents out of Markdown indentation", () => {
  for (const source of [
    "<b>safe</b>",
    "<details><summary>More</summary>text</details>",
  ])
    assert.equal(htmlPreview(source).code, false);
  for (const source of [
    "<script>run()</script>",
    "<style>b{color:red}</style>",
    '<div onclick="run()">x</div>',
    '<p style="color:red">x</p>',
    "<button>run</button>",
    '<iframe srcdoc="x"></iframe>',
  ])
    assert.equal(htmlPreview(source).code, true, source);
  const document =
    "<!doctype html>\n<html><head><title>Counter</title></head><body>\n\n    <div>\n        <h1>0</h1>\n    </div>\n</body></html>";
  assert.equal(htmlPreview(document).source, document);
  const nodes = flatten(markdownNodes(document, true));
  assert.ok(nodes.some((node) => node.tag === "h1"));
  assert.ok(!nodes.some((node) => node.tag === "pre"));
});

test("canonical root-relative attachments render while other local destinations stay blocked", () => {
  const path =
    "/commonroom/api/media/d5cd8a44-d8b7-4c63-91fe-3cc63f106178/file";
  assert.equal(safeMediaUrl(path), true);
  assert.equal(safeMediaUrl(path + "?unsafe=1"), false);
  const nodes = plainNodes(`![photo](${path}) [video](${path} "media:video")`);
  assert.match(JSON.stringify(nodes), /"tag":"img"/);
  assert.match(JSON.stringify(nodes), /"href":"\/commonroom\/api\/media\//);
  assert.equal(safeMediaUrl("/api/logout"), false);
});
