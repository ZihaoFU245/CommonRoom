import MarkdownIt from "markdown-it";
import { html, parseFragment } from "parse5";
import type { DefaultTreeAdapterTypes } from "parse5";
import { tex } from "@mdit/plugin-tex";
import { mathMarkdownNodes } from "./mathMarkdown.ts";
import { markdownNodes as plainNodes, markdownTokens } from "./markdown.ts";
import type { MarkdownNode } from "./markdown.ts";

const tags = new Set([
  "p",
  "strong",
  "em",
  "s",
  "a",
  "blockquote",
  "ul",
  "ol",
  "li",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "hr",
  "table",
  "thead",
  "tbody",
  "tr",
  "th",
  "td",
  "div",
  "span",
  "b",
  "i",
  "u",
  "del",
  "ins",
  "sub",
  "sup",
  "small",
  "mark",
  "abbr",
  "kbd",
  "samp",
  "details",
  "summary",
  "dl",
  "dt",
  "dd",
  "pre",
  "code",
  "br",
]);

const dangerous = new Set([
  "script",
  "style",
  "iframe",
  "object",
  "embed",
  "form",
  "svg",
  "math",
  "template",
  "noscript",
  "img",
  "video",
  "audio",
  "source",
  "picture",
  "track",
  "input",
  "button",
  "select",
  "textarea",
  "link",
  "meta",
  "base",
  "canvas",
  "title",
]);
export function markdownNodes(
  text: string,
  allowHtml = false,
  latex = false,
): MarkdownNode[] {
  if (!allowHtml) return latex ? mathMarkdownNodes(text) : plainNodes(text);
  const renderer = new MarkdownIt({ html: true, breaks: true });
  // Images use the same validated token converter as the light path; links are
  // independently filtered when rebuilding nodes below.
  renderer.validateLink = () => true;
  if (latex)
    renderer.use(tex, {
      delimiters: "all",
      render: (source: string) => source,
    });
  // The source cannot name this marker. Only token-renderer output can create it.
  let marker = "data-commonroom-generated";
  while (text.toLowerCase().includes(marker)) marker += "x";
  const generated: MarkdownNode[] = [];
  function placeholder(node: MarkdownNode): string {
    const index = generated.push(node) - 1;
    return `<span ${marker}="${index}"></span>`;
  }
  renderer.renderer.rules.image = (tokens, index) => {
    const token = tokens[index];
    return token ? markdownTokens([token]).map(placeholder).join("") : "";
  };
  if (latex) {
    renderer.renderer.rules.math_inline = (tokens, index) => {
      const token = tokens[index];
      return token ? markdownTokens([token]).map(placeholder).join("") : "";
    };
    renderer.renderer.rules.math_block = renderer.renderer.rules.math_inline;
  }
  function rebuild(nodes: DefaultTreeAdapterTypes.ChildNode[]): MarkdownNode[] {
    return nodes.flatMap((element): MarkdownNode[] => {
      if ("value" in element) return [element.value];
      if (
        !("tagName" in element) ||
        element.namespaceURI !== html.NS.HTML ||
        dangerous.has(element.tagName)
      )
        return [];
      const index = element.attrs.find((attr) => attr.name === marker)?.value;
      const node =
        index && /^\d+$/u.test(index) ? generated[Number(index)] : undefined;
      if (node) return [node];
      const children = rebuild(element.childNodes);
      if (!tags.has(element.tagName)) return children;
      const result: Exclude<MarkdownNode, string> = {
        tag: element.tagName,
        children,
      };
      if (result.tag === "a") {
        const href = element.attrs.find((attr) => attr.name === "href")?.value;
        if (href && (/^https?:\/\//iu.test(href) || /^mailto:/iu.test(href)))
          result.href = href;
        else result.tag = "span";
      }
      if (result.tag === "ol") {
        const start = element.attrs.find(
          (attr) => attr.name === "start",
        )?.value;
        if (start && /^-?\d+$/u.test(start)) result.start = start;
      }
      return [result];
    });
  }
  return rebuild(
    parseFragment(fullDocument(text) ? text : renderer.render(text)).childNodes,
  );
}

function fullDocument(text: string): boolean {
  return /^\s*(?:<!doctype\s+html\b|<html(?:\s|>))/iu.test(text);
}
const detector = new MarkdownIt({ html: true, breaks: true });
export function htmlPreview(
  text: string,
): { code: boolean; source: string } | null {
  const raw: string[] = [];
  function collect(tokens: ReturnType<typeof detector.parse>): void {
    for (const token of tokens) {
      if (token.type === "html_block" || token.type === "html_inline")
        raw.push(token.content);
      if (token.children) collect(token.children);
    }
  }
  collect(detector.parse(text, {}));
  if (!raw.length) return null;
  const active = new Set([
    "script",
    "style",
    "button",
    "input",
    "select",
    "textarea",
    "form",
    "canvas",
    "svg",
    "math",
    "iframe",
    "object",
    "embed",
    "link",
    "base",
    "img",
    "video",
    "audio",
    "source",
  ]);
  function code(nodes: DefaultTreeAdapterTypes.ChildNode[]): boolean {
    return nodes.some(
      (node) =>
        "tagName" in node &&
        (active.has(node.tagName) ||
          node.attrs.some(
            (attr) =>
              /^on/iu.test(attr.name) ||
              attr.name === "style" ||
              attr.name === "srcdoc" ||
              /^(?:javascript|vbscript):/iu.test(attr.value.trim()) ||
              (node.tagName === "meta" && attr.name === "http-equiv"),
          ) ||
          code(node.childNodes)),
    );
  }
  return {
    code: code(
      parseFragment(fullDocument(text) ? text : raw.join("\n")).childNodes,
    ),
    source: fullDocument(text) ? text : detector.render(text),
  };
}
