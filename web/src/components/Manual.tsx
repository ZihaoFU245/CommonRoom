import { createElement, type ComponentChildren } from "preact";
import { useMemo } from "preact/hooks";
import { commandNodes } from "../features/conversation/commandMarkdown.ts";
import type { MarkdownNode } from "../features/conversation/markdown.ts";

function render(nodes: MarkdownNode[]): ComponentChildren {
  return nodes.map((node, index) =>
    typeof node === "string"
      ? node
      : createElement(
          node.tag,
          {
            key: index,
            ...(node.href
              ? {
                  href: node.href,
                  target: "_blank",
                  rel: "noopener noreferrer",
                }
              : {}),
            ...(node.start ? { start: node.start } : {}),
          },
          render(node.children),
        ),
  );
}

export function Manual({
  text,
  foldSections = false,
}: {
  text: string;
  foldSections?: boolean;
}) {
  const nodes = useMemo(
    () => commandNodes(text, foldSections),
    [text, foldSections],
  );
  return <div class="manual">{render(nodes)}</div>;
}
