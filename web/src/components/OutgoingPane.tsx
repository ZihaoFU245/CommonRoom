import type { useOutgoingMessages } from "../hooks/useOutgoingMessages.ts";
export function OutgoingPane({
  items,
  selected,
  draft,
  restore,
}: {
  items: ReturnType<typeof useOutgoingMessages>["outgoing"];
  selected: string;
  draft: string;
  restore: (key: string) => void;
}) {
  const outgoing = items.filter((item) => item.view === selected);
  if (!outgoing.length) return null;
  return (
    <div class="outgoing-messages" aria-live="polite">
      {outgoing.map((item) => (
        <div key={item.key} class="outgoing-message">
          <p dir="auto">{item.text}</p>
          <small>{item.error ?? "Sending…"}</small>
          {item.error && (
            <button
              type="button"
              disabled={!!draft}
              onClick={() => restore(item.key)}
            >
              Edit and resend
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
