import type { LocalOutput } from "../api/protocol.ts";
import { outputMarkdown } from "../features/conversation/manual.ts";
import { lazy, Suspense } from "preact/compat";
import { useMemo } from "preact/hooks";

const Manual = lazy(() =>
  import("./Manual.tsx").then((module) => ({ default: module.Manual })),
);

export function ConsoleOutput({ entry }: { entry: LocalOutput }) {
  const text = useMemo(
    () => outputMarkdown(entry),
    [entry.command, entry.result, entry.error, entry.room],
  );
  return (
    <article
      class={`console-output ${entry.error ? "error" : ""}`}
      data-local-output="true"
    >
      <div class="console-input">
        <span aria-hidden="true">›</span>
        <code>{entry.command}</code>
      </div>
      <Suspense fallback={<pre>{entry.result ?? "…"}</pre>}>
        <Manual text={text} />
      </Suspense>
    </article>
  );
}
