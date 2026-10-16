import { createElement } from "preact";
import type { ComponentChildren } from "preact";
import { useMemo } from "preact/hooks";
import { parseFragment } from "parse5";
import type { DefaultTreeAdapterTypes } from "parse5";
import { mathMarkup } from "../features/conversation/math.ts";
import "katex/dist/katex.min.css";

function render(nodes: DefaultTreeAdapterTypes.ChildNode[]): ComponentChildren {
  return nodes.map((node, index) => {
    if ("value" in node) return node.value;
    if (!("tagName" in node)) return null;
    const props: Record<string, string | number | Record<string, string>> = {
      key: index,
    };
    for (const attr of node.attrs) {
      if (attr.name === "style") {
        // CSSOM properties keep generated math styles working under the chat's
        // strict CSP. Only KaTeX output, never message HTML, takes this path.
        const style: Record<string, string> = {};
        for (const declaration of attr.value.split(";")) {
          const colon = declaration.indexOf(":");
          if (colon > 0)
            style[declaration.slice(0, colon).trim()] = declaration
              .slice(colon + 1)
              .trim();
        }
        props["style"] = style;
      } else props[attr.name] = attr.value;
    }
    return createElement(node.tagName, props, render(node.childNodes));
  });
}

export function Math({
  source,
  display,
}: {
  source: string;
  display: boolean;
}) {
  const nodes = useMemo(
    () => parseFragment(mathMarkup(source, display)).childNodes,
    [source, display],
  );
  return <>{render(nodes)}</>;
}
