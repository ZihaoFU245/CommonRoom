import MarkdownIt from "markdown-it";
import { tex } from "@mdit/plugin-tex";
import { markdownTokens } from "./markdown.ts";
const parser = new MarkdownIt({ html: false, breaks: true }).use(tex, {
  delimiters: "all",
  render: (source: string) => source,
});
parser.validateLink = () => true;
export function mathMarkdownNodes(text: string) {
  return markdownTokens(parser.parse(text, {}));
}
