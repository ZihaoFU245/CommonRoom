import { createElement } from "preact";
import type { ComponentChildren } from "preact";
import { markdownNodes } from "../features/conversation/markdown.ts";
import type { MarkdownNode } from "../features/conversation/markdown.ts";
import { mentionedText } from "../features/conversation/interactions.ts";

export function Markdown({
  text,
  mentions,
}: {
  text: string;
  mentions: string[];
}) {
  function render(nodes: MarkdownNode[], code = false): ComponentChildren {
    return nodes.map((node, index) => {
      if (typeof node === "string")
        return code
          ? node
          : mentionedText(node, mentions).map((part, i) =>
              part.mention ? (
                <mark key={i} class="mention">
                  {part.text}
                </mark>
              ) : (
                part.text
              ),
            );
      return createElement(
        node.tag,
        {
          key: index,
          ...(node.href
            ? { href: node.href, target: "_blank", rel: "noopener noreferrer" }
            : {}),
          ...(node.start ? { start: node.start } : {}),
        },
        render(node.children, code || node.tag === "code"),
      );
    });
  }
  return (
    <div dir="auto" class="message-markdown">
      {render(markdownNodes(text))}
    </div>
  );
}
