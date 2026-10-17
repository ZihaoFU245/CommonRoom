import { createElement } from "preact";
import type { ComponentChildren } from "preact";
import { lazy, memo, Suspense } from "preact/compat";
import type { MarkdownNode } from "../features/conversation/markdown.ts";
import { mentionedText } from "../features/conversation/interactions.ts";
const Math = lazy(() =>
  import("./Math.tsx")
    .then((module) => ({ default: module.Math }))
    .catch(() => ({
      default: ({ source }: { source: string }) => <>{source}</>,
    })),
);
export const MarkdownContent = memo(function MarkdownContent({
  nodes,
  mentions,
  renderHtml,
}: {
  nodes: MarkdownNode[];
  mentions: string[];
  renderHtml?: (source: string, index: number) => ComponentChildren;
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
      if (node.html)
        return (
          renderHtml?.(node.html.source, node.html.index) ?? node.html.source
        );
      if (node.math)
        return (
          <Suspense key={index} fallback={node.math.source}>
            <Math source={node.math.source} display={node.math.display} />
          </Suspense>
        );
      return createElement(
        node.tag,
        {
          key: index,
          ...(node.href
            ? { href: node.href, target: "_blank", rel: "noopener noreferrer" }
            : {}),
          ...(node.start ? { start: node.start } : {}),
          ...(node.image
            ? {
                src: node.image.src,
                alt: node.image.alt,
                loading: "lazy",
                decoding: "async",
                referrerPolicy: "no-referrer",
              }
            : {}),
        },
        render(node.children, code || node.tag === "code"),
      );
    });
  }
  return (
    <div dir="auto" class="message-markdown">
      {render(nodes)}
    </div>
  );
});
