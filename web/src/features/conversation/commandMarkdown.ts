import MarkdownIt from "markdown-it";
import { markdownTokens, type MarkdownNode } from "./markdown.ts";

// Documentation cannot load images, activate attachments, or run HTML previews.
// This parser lives with the lazy command-output renderer, outside the app shell.
const parser = new MarkdownIt({ html: false, breaks: false }).disable("image");
export function commandNodes(text: string): MarkdownNode[] {
  return markdownTokens(parser.parse(text, {}));
}
