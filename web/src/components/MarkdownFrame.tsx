import { useLayoutEffect, useRef, useState } from "preact/hooks";
import frameUrl from "virtual:markdown-frame-url";
import { frameSandbox } from "../features/conversation/framePolicy.ts";
export function MarkdownFrame({ source }: { source: string }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const sent = useRef(false);
  const [height, setHeight] = useState(360);
  const [error, setError] = useState(false);
  const [attempt, retry] = useState(0);
  useLayoutEffect(() => {
    sent.current = false;
    setError(false);
    const timeout = setTimeout(() => setError(true), 10000);
    const resize = (event: MessageEvent<unknown>) => {
      if (
        event.source !== frame.current?.contentWindow ||
        event.origin !== "null" ||
        !event.data ||
        typeof event.data !== "object" ||
        !("kind" in event.data) ||
        event.data.kind !== "markdown-height" ||
        !("height" in event.data) ||
        typeof event.data.height !== "number" ||
        !Number.isFinite(event.data.height)
      )
        return;
      clearTimeout(timeout);
      setHeight(Math.max(128, Math.min(800, event.data.height)));
    };
    addEventListener("message", resize);
    return () => {
      clearTimeout(timeout);
      removeEventListener("message", resize);
    };
  }, [attempt]);
  return (
    <>
      <iframe
        key={attempt}
        ref={frame}
        title="HTML preview"
        class="markdown-frame"
        sandbox={frameSandbox}
        referrerPolicy="no-referrer"
        src={frameUrl}
        style={{ height }}
        onLoad={() => {
          if (sent.current) return;
          sent.current = true;
          frame.current?.contentWindow?.postMessage(
            { kind: "markdown-render", source },
            "*",
          );
        }}
      />
      {error && (
        <>
          <p role="status">
            Unable to load HTML preview.{" "}
            <button type="button" onClick={() => retry((value) => value + 1)}>
              Retry
            </button>
          </p>
        </>
      )}
    </>
  );
}
