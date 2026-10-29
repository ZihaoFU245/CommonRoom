import { mediaPath } from "../../api/media.ts";
import MarkdownIt from "markdown-it";
import { safeMediaUrl } from "./mediaUrl.ts";
type Token = ReturnType<typeof parser.parse>[number];

const parser = new MarkdownIt({ html: false, breaks: true });
// Tokenize rejected destinations too, so image URLs fall back to their alt text.
// The node converter below owns link/image URL validation on every output path.
parser.validateLink = () => true;

let htmlDetector: typeof parser | undefined;
export function hasRawHtml(text: string): boolean {
  if (!text.includes("<")) return false;
  htmlDetector ??= new MarkdownIt({ html: true, breaks: true });
  function contains(tokens: Token[]): boolean {
    return tokens.some(
      (token) =>
        token.type === "html_block" ||
        token.type === "html_inline" ||
        (token.children !== null && contains(token.children)),
    );
  }
  return contains(htmlDetector.parse(text, {}));
}

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
      title?: string;
      start?: string;
      math?: { source: string; display: boolean };
      image?: { src: string; alt: string; title?: string };
      html?: { source: string; index: number };
    };
export function markdownTokens(tokens: Token[]): MarkdownNode[] {
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
          if (
            /^https?:\/\//iu.test(href) ||
            /^mailto:/iu.test(href) ||
            mediaPath(href)
          )
            node.href = href;
          else node.tag = "span";
          const title = token.attrGet("title");
          if (title) node.title = String(title);
        }
        if (node.tag === "ol") {
          const start = token.attrGet("start");
          if (start) node.start = String(start);
        }
        current.push(node);
        stack.push(node.children);
      } else if (token.type === "image") {
        const src = String(token.attrGet("src") ?? "");
        const alt = parser.renderer.renderInlineAsText(
          token.children ?? [],
          parser.options,
          {},
        );
        const title = token.attrGet("title");
        current.push(
          safeMediaUrl(src)
            ? {
                tag: "img",
                children: [],
                image: title
                  ? { src, alt, title: String(title) }
                  : { src, alt },
              }
            : alt,
        );
      } else if (token.type === "inline") {
        current.push(...convert(token.children ?? []));
      } else if (token.type === "math_inline" || token.type === "math_block") {
        const display = token.type === "math_block";
        current.push({
          tag: display ? "div" : "span",
          children: [],
          math: { source: token.content, display },
        });
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
  return convert(tokens);
}
export function markdownNodes(text: string): MarkdownNode[] {
  return markdownTokens(parser.parse(text, {}));
}
