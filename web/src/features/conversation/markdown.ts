import MarkdownIt from "markdown-it";
type Token = ReturnType<typeof parser.parse>[number];
import type { Command } from "../../api/protocol.ts";

const parser = new MarkdownIt({ html: false, breaks: true });
// Images stay as alt text: messages never fetch remote tracking images.

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
]);
export type MarkdownNode =
  | string
  | {
      tag: string;
      children: MarkdownNode[];
      href?: string;
      start?: string;
    };
export function markdownNodes(text: string): MarkdownNode[] {
  function convert(tokens: Token[]): MarkdownNode[] {
    const root: MarkdownNode[] = [];
    const stack = [root];
    for (const token of tokens) {
      const current = stack.at(-1) ?? root;
      if (token.nesting === -1) {
        if (!token.hidden) stack.pop();
      } else if (token.nesting === 1) {
        if (token.hidden) continue;
        const node: Exclude<MarkdownNode, string> = {
          tag: tags.has(token.tag) ? token.tag : "span",
          children: [],
        };
        if (node.tag === "a") {
          const href = String(token.attrGet("href") ?? "");
          if (/^https?:\/\//iu.test(href) || /^mailto:/iu.test(href))
            node.href = String(href);
          else node.tag = "span";
        }
        if (node.tag === "ol") {
          const start = token.attrGet("start");
          if (start) node.start = String(start);
        }
        current.push(node);
        stack.push(node.children);
      } else if (token.type === "inline") {
        current.push(...convert(token.children ?? []));
      } else if (token.type === "fence" || token.type === "code_block") {
        current.push({
          tag: "pre",
          children: [{ tag: "code", children: [token.content] }],
        });
      } else if (token.type === "code_inline") {
        current.push({ tag: "code", children: [token.content] });
      } else if (token.type === "softbreak" || token.type === "hardbreak") {
        current.push({ tag: "br", children: [] });
      } else if (token.type === "hr") {
        current.push({ tag: "hr", children: [] });
      } else {
        current.push(token.content);
      }
    }
    return root;
  }
  return convert(parser.parse(text, {}));
}
export const markdownCommand: Command = {
  name: "/markdown",
  usage: "/markdown on|off",
  description: "Format future messages with Markdown in this browser",
  requirements: "",
  section: "General",
  admin: false,
  console: false,
};
export const markdownManual = `# /markdown

Use /markdown on to format future room messages, private messages and replies.
Use /markdown off to send plain text again. The default is off.

Supports bold, italic, strikethrough, headings, lists, quotes, code and links.
Raw HTML and embedded images are disabled. Links allow HTTP, HTTPS and mailto.

The setting stays in this browser. Each sent message keeps its own format,
including after refresh; changing the setting does not change earlier messages.`;
const key = "commonroom.markdown";
export function loadMarkdown(): boolean {
  try {
    return localStorage.getItem(key) === "on";
  } catch {
    return false;
  }
}
export function setMarkdown(input: string): boolean {
  if (!/^\/markdown (on|off)$/u.test(input))
    throw new Error("Usage: /markdown on|off");
  const enabled = input === "/markdown on";
  try {
    localStorage.setItem(key, enabled ? "on" : "off");
  } catch {
    /* Keep the session preference when storage is unavailable. */
  }
  return enabled;
}
