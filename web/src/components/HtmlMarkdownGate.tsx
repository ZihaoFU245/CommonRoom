import type { ComponentChildren } from "preact";
import { lazy, Suspense } from "preact/compat";
import { useEffect, useState } from "preact/hooks";
import { loadHtmlRenderer } from "./loadHtmlRenderer.ts";
import { MarkdownContent } from "./MarkdownContent.tsx";
import type { MarkdownProps } from "./Markdown.tsx";
import type { MarkdownNode } from "../features/conversation/markdown.ts";
import { HtmlCover } from "./HtmlCover.tsx";
const MarkdownFrame = lazy(() =>
  import("./MarkdownFrame.tsx")
    .then((module) => ({
      default: module.MarkdownFrame,
    }))
    .catch(() => ({
      default: () => (
        <p role="status">Unable to load HTML frame. Reload to retry.</p>
      ),
    })),
);
export function HtmlMarkdownGate(
  props: MarkdownProps & { fallback: ComponentChildren },
) {
  const {
    text,
    mentions,
    latex,
    allowHtml,
    allowHtmlCode,
    messageKey,
    fallback,
  } = props;
  const [attempt, retry] = useState(0);
  const [result, setResult] = useState<{
    text: string;
    latex: boolean;
    allowHtml: boolean;
    allowHtmlCode: boolean;
    preview: { code: boolean; source: string } | null;
    nodes: MarkdownNode[];
  } | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setError(false);
    loadHtmlRenderer()
      .then((module) => {
        if (cancelled) return;
        const preview = module.htmlPreview(text);
        // Code never enters the main DOM. Secure mode keeps only passive nodes.
        const nodes =
          preview && !(preview.code && allowHtmlCode)
            ? module.markdownNodes(text, true, latex)
            : [];
        setResult({ text, latex, allowHtml, allowHtmlCode, preview, nodes });
      })
      .catch(() => {
        if (!cancelled) setError(true);
      });
    return () => {
      cancelled = true;
    };
  }, [text, latex, allowHtml, allowHtmlCode, attempt]);
  const current =
    result?.text === text &&
    result.latex === latex &&
    result.allowHtml === allowHtml &&
    result.allowHtmlCode === allowHtmlCode
      ? result
      : null;
  if (!current) {
    const status = error ? (
      <p role="status">
        Unable to load HTML renderer.{" "}
        <button type="button" onClick={() => retry((value) => value + 1)}>
          Retry
        </button>
      </p>
    ) : (
      <p role="status">Loading HTML preview…</p>
    );
    return allowHtmlCode ? (
      <HtmlCover text={text} messageKey={messageKey}>
        {status}
      </HtmlCover>
    ) : (
      status
    );
  }
  if (!current.preview) return fallback;
  const framed = current.preview.code && allowHtmlCode;
  const manual = framed || !allowHtml;
  const direct = current.nodes.length ? (
    <MarkdownContent nodes={current.nodes} mentions={mentions} />
  ) : (
    <>
      {fallback}
      <p role="status">No safe HTML to display</p>
    </>
  );
  const loading = <p role="status">Loading HTML preview…</p>;
  const content = framed ? (
    <Suspense fallback={loading}>
      <MarkdownFrame key={text} source={current.preview.source} />
    </Suspense>
  ) : (
    direct
  );
  if (!manual) return content;
  return (
    <HtmlCover
      text={text}
      messageKey={messageKey}
      label={framed ? "HTML code" : "HTML content"}
    >
      {content}
    </HtmlCover>
  );
}
