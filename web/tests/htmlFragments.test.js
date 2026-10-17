import test from "node:test";
import assert from "node:assert/strict";
import {
  htmlFragments,
  restoreHtmlFragments,
} from "../src/features/conversation/htmlFragments.ts";
import { markdownNodes } from "../src/features/conversation/markdown.ts";
import { mathMarkdownNodes } from "../src/features/conversation/mathMarkdown.ts";

function flatten(nodes) {
  return nodes.flatMap((node) =>
    typeof node === "string" ? [node] : [node, ...flatten(node.children)],
  );
}
function render(source, math = false) {
  const parts = htmlFragments(source);
  return flatten(
    restoreHtmlFragments(
      (math ? mathMarkdownNodes : markdownNodes)(parts.markdown),
      parts.fragments,
    ),
  );
}

test("mixed Markdown keeps formatting and isolates only the HTML source", () => {
  const nodes = render(
    "**Markdown test**\n\n<html>haha</html>\n\n**markdown**",
  );
  assert.deepEqual(
    nodes.filter((node) => node.tag === "strong").map((node) => node.children),
    [["Markdown test"], ["markdown"]],
  );
  assert.deepEqual(
    nodes.filter((node) => node.html).map((node) => node.html.source),
    ["<html>haha</html>"],
  );
});

test("inline formatting, lists, quotes and math survive HTML extraction", () => {
  for (const text of [
    "**before <b>inside</b> after**",
    "- before <b>inside</b> after",
    "> before <b>inside</b> after",
    "# before <b>inside</b> after",
  ]) {
    const nodes = render(text);
    assert.deepEqual(
      nodes.filter((node) => node.html).map((node) => node.html.source),
      ["<b>inside</b>"],
    );
    assert.ok(
      nodes.some((node) =>
        ["strong", "li", "blockquote", "h1"].includes(node.tag),
      ),
    );
  }
  const nodes = render("$x^2$ <span>HTML</span> **after**", true);
  assert.ok(nodes.some((node) => node.math?.source === "x^2"));
  assert.ok(nodes.some((node) => node.tag === "strong"));
  assert.ok(nodes.some((node) => node.html?.source === "<span>HTML</span>"));
});

test("fences, inline code, escaped tags, autolinks and image alt stay literal", () => {
  for (const text of [
    "`<b>literal</b>`",
    "``<b>`literal`</b>``",
    "```html\n<b>literal</b>\n```",
    "    <div>literal</div>",
    "\\<b>literal",
    "<https://example.com>",
    "<a@example.com>",
    "![<b>alt</b>](https://example.com/image.png)",
  ]) {
    assert.deepEqual(htmlFragments(text).fragments, [], text);
  }
  const parts = htmlFragments("<div>real</div>\n`<b>literal</b>`");
  assert.deepEqual(
    parts.fragments.map((part) => part.source),
    ["<div>real</div>"],
  );
  assert.ok(parts.markdown.includes("`<b>literal</b>`"));
});

test("nested HTML and full documents remain intact without swallowing later Markdown", () => {
  for (const html of [
    '<div title="a > b"><div>nested</div><span>end</span></div>',
    '<script>const text = "<div>fake</div>";</script>',
    '<div><script>const text = "</div>";</script><b>end</b></div>',
    "<!-- <div>comment</div> -->",
    "<!doctype html>\n<html><head><style>body { color: red; }</style></head>\n<body>\n\n    <button>click</button>\n</body></html>",
  ]) {
    const parts = htmlFragments(`**before**\n\n${html}\n\n**after**`);
    assert.deepEqual(
      parts.fragments.map((part) => part.source),
      [html],
    );
    assert.ok(parts.markdown.startsWith("**before**"));
    assert.ok(parts.markdown.endsWith("**after**"));
  }
});

test("multiple previews preserve order and cannot collide with message text", () => {
  const text =
    "COMMONROOMHTMLFRAGMENT0END\n\n**one** <b>first</b> and <i>second</i> **two**";
  const parts = htmlFragments(text);
  assert.deepEqual(
    parts.fragments.map((part) => part.source),
    ["<b>first</b>", "<i>second</i>"],
  );
  assert.ok(parts.markdown.includes("COMMONROOMHTMLFRAGMENT0END"));
  assert.ok(parts.fragments.every((part) => !text.includes(part.marker)));
  assert.deepEqual(
    render(text)
      .filter((node) => node.html)
      .map((node) => node.html.index),
    [0, 1],
  );
});

test("adjacent HTML, CSS and scripts share one working preview", () => {
  const html =
    '<style>#x { color: red; }</style>\n\n<div id="x">0</div>\n<script>document.querySelector("#x").textContent = 1;</script>';
  const parts = htmlFragments(`**before**\n\n${html}\n\n**after**`);
  assert.deepEqual(
    parts.fragments.map((part) => part.source),
    [html],
  );
  assert.ok(parts.markdown.endsWith("**after**"));
});

test("indented full HTML documents bypass Markdown code-block parsing", () => {
  const source =
    "    <!doctype html>\n    <html>\n      <body>hello</body>\n    </html>";
  const nodes = render(source);
  assert.deepEqual(
    nodes.filter((node) => node.html).map((node) => node.html.source),
    [source],
  );
  assert.ok(!nodes.some((node) => node.tag === "code" || node.tag === "pre"));
});
