import type { ComponentChildren } from "preact";
import { lazy, Suspense } from "preact/compat";
import { useEffect, useState } from "preact/hooks";
import { loadHtmlRenderer } from "./loadHtmlRenderer.ts";
import type { HtmlCoverProps } from "./HtmlCover.tsx";
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
  props: HtmlCoverProps & { fallback: ComponentChildren },
) {
  const { text, fallback } = props;
  const [attempt, retry] = useState(0);
  const [result, setResult] = useState<{
    text: string;
    preview: { code: boolean; source: string } | null;
  } | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    let cancelled = false;
    setError(false);
    loadHtmlRenderer()
      .then((module) => {
        if (cancelled) return;
        const preview = module.htmlPreview(text);
        setResult({ text, preview });
      })
      .catch(() => {
        if (!cancelled) setError(true);
      });
    return () => {
      cancelled = true;
    };
  }, [text, attempt]);
  const current = result?.text === text ? result : null;
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
    return <HtmlCover {...props}>{status}</HtmlCover>;
  }
  if (!current.preview) return fallback;
  return (
    <HtmlCover {...props}>
      <Suspense fallback={<p role="status">Loading HTML preview…</p>}>
        <MarkdownFrame key={text} source={current.preview.source} />
      </Suspense>
    </HtmlCover>
  );
}
