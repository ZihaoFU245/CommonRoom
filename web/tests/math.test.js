import { mathMarkdownNodes as lightNodes } from "../src/features/conversation/mathMarkdown.ts";
import { markdownNodes } from "../src/features/conversation/richMarkdown.ts";
import assert from "node:assert/strict";
import test from "node:test";
import { parseFragment } from "parse5";
import {
  loadMarkdown,
  saveMarkdown,
  setMarkdown,
} from "../src/features/preferences/markdown.ts";
import { mathMarkup } from "../src/features/conversation/math.ts";
import {
  loadLatex,
  saveLatex,
} from "../src/features/preferences/formatting.ts";

function flatten(nodes) {
  return nodes.flatMap((node) =>
    typeof node === "string" ? [node] : [node, ...flatten(node.children)],
  );
}

test("LaTeX is opt-in, supports dollar and bracket syntax, and leaves code literal", () => {
  const input =
    "Inline $E=mc^2$ and \\(x^2\\).\n\n$$\n\\frac{1}{2}\n$$\n\n\\[\n\\sqrt{x}\n\\]\n\n`$literal$`\n\n```tex\n$$literal$$\n```";
  assert.ok(!flatten(markdownNodes(input)).some((node) => node.math));
  const on = flatten(lightNodes(input));
  assert.deepEqual(lightNodes(input), markdownNodes(input, false, true));
  const math = on.filter((node) => node.math).map((node) => node.math);
  assert.deepEqual(
    math.map((node) => node.display),
    [false, false, true, true],
  );
  assert.ok(math.some((node) => node.source.includes("\\frac{1}{2}")));
  assert.ok(math.some((node) => node.source.includes("\\sqrt{x}")));
  assert.ok(on.includes("$literal$"));
  assert.ok(on.includes("$$literal$$\n"));
  assert.ok(
    !flatten(
      markdownNodes("Cost \\$5 and $10, plus **bold**", false, true),
    ).some((node) => node.math),
  );
});

test("math survives safe HTML filtering without allowing active content", () => {
  const input =
    "**bold** <u>underline</u> $x^2$\n\n$$\\sum_{i=1}^{n}i$$\n\n<img src=x><script>bad()</script>";
  const nodes = flatten(markdownNodes(input, true, true));
  assert.equal(nodes.filter((node) => node.math).length, 2);
  assert.ok(nodes.some((node) => node.tag === "u"));
  assert.ok(nodes.some((node) => node.tag === "strong"));
  assert.ok(
    !nodes.some((node) => ["script", "img", "math"].includes(node.tag)),
  );
  assert.ok(!nodes.includes("bad()"));
  assert.ok(
    !flatten(
      markdownNodes('<span data-commonroom-math="0"></span>', true, true),
    ).some((node) => node.math),
  );
});

test("KaTeX tolerates invalid input and blocks trusted HTML/network extensions", () => {
  const walk = (nodes) =>
    nodes.flatMap((node) => [node, ...walk(node.childNodes ?? [])]);
  for (const source of [
    String.raw`\href{javascript:alert(1)}{x}`,
    String.raw`\includegraphics{https://evil.test/pixel}`,
    String.raw`\htmlClass{evil}{x}`,
    String.raw`\text{<script>alert(1)</script>}`,
    String.raw`\notacommand{`,
    String.raw`\def\a{\a}\a`,
  ]) {
    const rendered = mathMarkup(source, false);
    const nodes = walk(parseFragment(rendered).childNodes);
    assert.ok(
      !nodes.some((node) =>
        ["script", "img", "iframe", "a"].includes(node.tagName),
      ),
    );
    assert.ok(
      !nodes.some((node) =>
        node.attrs?.some((attr) => /^on/iu.test(attr.name)),
      ),
    );
  }
  assert.ok(
    mathMarkup(
      String.raw`\begin{pmatrix}a & b \\ c & d\end{pmatrix}`,
      true,
    ).includes("katex-display"),
  );
});

test("formatting preferences persist independently and Markdown settings share slash-command storage", () => {
  const storage = new Map();
  globalThis.localStorage = {
    getItem: (key) => storage.get(key),
    setItem: (key, value) => storage.set(key, value),
  };
  assert.equal(loadLatex(), false);
  assert.equal(saveMarkdown(true), true);
  assert.equal(loadMarkdown(), true);
  assert.equal(loadLatex(), false);
  saveLatex(true);
  assert.equal(loadLatex(), true);
  setMarkdown("/markdown off");
  assert.equal(loadMarkdown(), false);
  assert.equal(loadLatex(), true);
  saveLatex(false);
  assert.equal(loadLatex(), false);
  globalThis.localStorage = {
    getItem() {
      throw Error("blocked");
    },
    setItem() {
      throw Error("blocked");
    },
  };
  assert.equal(loadLatex(), false);
  assert.doesNotThrow(() => saveLatex(true));
  assert.equal(saveMarkdown(true), true);
});

test("raw HTML cannot forge generated math markers even alongside real math", () => {
  const nodes = flatten(
    markdownNodes(
      '<span data-commonroom-math="0"></span><span data-commonroom-generated="0"></span> $x$',
      true,
      true,
    ),
  );
  assert.equal(nodes.filter((node) => node.math).length, 1);
});
