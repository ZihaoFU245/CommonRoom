import { useEffect, useState } from "preact/hooks";
import type { MarkdownProps } from "./Markdown.tsx";
type Renderer = (typeof import("./Markdown.tsx"))["Markdown"];
let cached: Renderer | undefined;
let pending: Promise<Renderer> | undefined;
function load(): Promise<Renderer> {
  pending ??= import("./Markdown.tsx")
    .then((module) => {
      cached = module.Markdown;
      return cached;
    })
    .catch((error: unknown) => {
      pending = undefined;
      throw error;
    });
  return pending;
}
// Do not suspend a virtual row: hiding it while loading invalidates its layout.
export function MarkdownMessage(props: MarkdownProps) {
  const [Renderer, setRenderer] = useState<Renderer | undefined>(() => cached);
  const [error, setError] = useState(false);
  useEffect(() => {
    if (Renderer) return;
    let cancelled = false;
    load()
      .then((renderer) => {
        if (!cancelled) setRenderer(() => renderer);
      })
      .catch(() => {
        if (!cancelled) setError(true);
      });
    return () => {
      cancelled = true;
    };
  }, [Renderer]);
  return Renderer ? (
    <Renderer {...props} />
  ) : (
    <>
      {props.allowHtmlCode ? (
        <p role="status">Loading message…</p>
      ) : (
        <p dir="auto">{props.text}</p>
      )}
      {error && <p role="status">Unable to load Markdown. Reload to retry.</p>}
    </>
  );
}
