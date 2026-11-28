import { createElement, type ComponentChildren } from "preact";
import { useEffect, useId, useMemo, useState } from "preact/hooks";
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

function MetricsCodeBlock({ report }: { report: string }) {
  const id = useId();
  const [feedback, setFeedback] = useState("");
  useEffect(() => {
    if (!feedback) return;
    const timer = window.setTimeout(() => setFeedback(""), 2000);
    return () => window.clearTimeout(timer);
  }, [feedback]);

  function showFeedback(message: string) {
    setFeedback((previous) =>
      previous === message ? `${message}\u00a0` : message,
    );
  }

  function copy() {
    const clipboard = navigator.clipboard;
    if (!clipboard) {
      showFeedback("Couldn’t copy");
      return;
    }
    void clipboard.writeText(report).then(
      () => showFeedback("Copied"),
      () => showFeedback("Couldn’t copy"),
    );
  }

  return (
    <div class="metrics-code-block">
      <pre id={id} onCommand={copy}>
        <code>{report}</code>
      </pre>
      <div class="metrics-code-actions">
        <span role="status">{feedback}</span>
        <button
          type="button"
          command="--copy"
          commandfor={id}
          onClick={
            "commandForElement" in HTMLButtonElement.prototype
              ? undefined
              : copy
          }
        >
          Copy metrics
        </button>
      </div>
    </div>
  );
}

export function Manual({
  text,
  foldSections = false,
  copyMetrics = false,
}: {
  text: string;
  foldSections?: boolean;
  copyMetrics?: boolean;
}) {
  const nodes = useMemo(
    () => commandNodes(text, foldSections),
    [text, foldSections],
  );
  const [block] = nodes;
  if (
    copyMetrics &&
    nodes.length === 1 &&
    typeof block === "object" &&
    block.tag === "pre"
  ) {
    const [code] = block.children;
    if (typeof code === "object" && code.tag === "code") {
      const [report] = code.children;
      if (typeof report === "string") {
        return (
          <div class="manual">
            <MetricsCodeBlock report={report} />
          </div>
        );
      }
    }
  }
  return <div class="manual">{render(nodes)}</div>;
}
