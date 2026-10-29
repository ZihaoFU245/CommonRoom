import type { LocalOutput } from "../api/protocol.ts";
import { outputMarkdown } from "../features/conversation/manual.ts";
import { commandText } from "../features/conversation/console.ts";
import { useEffect, useMemo, useState } from "preact/hooks";

type Renderer = (typeof import("./Manual.tsx"))["Manual"];
let cached: Renderer | undefined;
let pending: Promise<Renderer> | undefined;
function load(): Promise<Renderer> {
  pending ??= import("./Manual.tsx")
    .then((module) => {
      cached = module.Manual;
      return cached;
    })
    .catch((error: unknown) => {
      pending = undefined;
      throw error;
    });
  return pending;
}

export function ConsoleOutput({ entry }: { entry: LocalOutput }) {
  // Suspending a virtual row hides its content and can make synchronous
  // measurements alternate between fallback and content heights indefinitely.
  const [Manual, setManual] = useState<Renderer | undefined>(() => cached);
  const [error, setError] = useState(false);
  useEffect(() => {
    if (Manual) return;
    let cancelled = false;
    load()
      .then((renderer) => {
        if (!cancelled) setManual(() => renderer);
      })
      .catch(() => {
        if (!cancelled) setError(true);
      });
    return () => {
      cancelled = true;
    };
  }, [Manual]);
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
      {Manual ? (
        <Manual
          text={text}
          foldSections={
            !entry.error &&
            /^\/system\s+metrics$/u.test(commandText(entry.command))
          }
        />
      ) : (
        <pre>{entry.result ?? "…"}</pre>
      )}
      {error && (
        <p role="status">Unable to load formatting. Reload to retry.</p>
      )}
    </article>
  );
}
