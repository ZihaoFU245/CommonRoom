import type { LocalOutput } from "../api/protocol.ts";
import { helpSections } from "../features/conversation/console.ts";

export function ConsoleOutput({ entry }: { entry: LocalOutput }) {
  const help = entry.command === "/help" && !entry.error && entry.result;
  return (
    <article
      class={`console-output ${entry.error ? "error" : ""}`}
      data-local-output="true"
    >
      <div class="console-input">
        <span aria-hidden="true">›</span>
        <code>{entry.command}</code>
      </div>
      {help ? (
        <div class="help-sections">
          {helpSections(entry.result!).map((group) => (
            <section key={group.title}>
              <h2>{group.title}</h2>
              <ul class="help-list" aria-label={`${group.title} commands`}>
                {group.commands.map(({ usage, description }) => (
                  <li key={usage}>
                    <code>{usage}</code>
                    <span>{description}</span>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      ) : (
        <pre>{entry.result ?? "…"}</pre>
      )}
    </article>
  );
}
