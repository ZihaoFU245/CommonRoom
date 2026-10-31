import { DisplayName } from "./DisplayName.tsx";
import { useEffect, useRef, useState } from "preact/hooks";
import { useNameColor } from "./NameColorContext.ts";

function OnlineName({ name, self }: { name: string; self: boolean }) {
  return (
    <span>
      <b data-name-color={useNameColor(name)}>
        <DisplayName name={name} />
      </b>
      {self ? " (you)" : ""}
    </span>
  );
}

export function OnlineUsers({
  names,
  username,
  connected,
}: {
  names: string[];
  username: string;
  connected: boolean;
}) {
  const [open, setOpen] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !container.current?.contains(event.target)
      )
        setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      setOpen(false);
      button.current?.focus();
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  return (
    <div class={`online-users ${open ? "open" : ""}`} ref={container}>
      <button
        class="online-toggle"
        ref={button}
        aria-expanded={open}
        aria-controls="online-users-list"
        onClick={() => setOpen(!open)}
      >
        {connected ? `${names.length} online` : "Online users"}
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="m6 9 6 6 6-6" />
        </svg>
      </button>
      <div id="online-users-list" class="online-list" aria-hidden={!open}>
        {connected ? (
          names.map((name) => (
            <OnlineName key={name} name={name} self={name === username} />
          ))
        ) : (
          <span>Reconnecting…</span>
        )}
      </div>
    </div>
  );
}
