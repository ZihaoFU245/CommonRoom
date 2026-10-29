import MarkdownIt from "markdown-it";
import { markdownTokens, type MarkdownNode } from "./markdown.ts";

// Documentation cannot load images, activate attachments, or run HTML previews.
// This parser lives with the lazy command-output renderer, outside the app shell.
const parser = new MarkdownIt({ html: false, breaks: false }).disable("image");
export function commandNodes(
  text: string,
  foldSections = false,
): MarkdownNode[] {
  const nodes = markdownTokens(parser.parse(text, {}));
  if (!foldSections) return nodes;
  const result: MarkdownNode[] = [];
  let section: MarkdownNode[] | undefined;
  for (const node of nodes) {
    if (typeof node !== "string" && node.tag === "h2") {
      section = [{ tag: "summary", children: node.children }];
      result.push({ tag: "details", children: section });
    } else {
      (section ?? result).push(node);
    }
  }
  return result;
}
