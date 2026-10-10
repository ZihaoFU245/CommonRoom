import { useEffect, useState } from "preact/hooks";
import type { Snapshot } from "../api/protocol.ts";

/**
 * Edit one agent's personality. A personality is free text, so the composer
 * command is a fallback; this dialog is the readable way to write one.
 */
export function AgentPrompt({
  state,
  onSend,
  onClose,
}: {
  state: Snapshot;
  onSend: (text: string) => void;
  onClose: () => void;
}) {
  const owned = state.users.filter(
    (name) =>
      state.roles?.[name] === "agent" &&
      (state.admin || state.prompts?.[name] !== undefined),
  );
  const [agent, setAgent] = useState(() => owned[0] || "");
  const [text, setText] = useState(() => state.prompts?.[owned[0] || ""] || "");

  useEffect(() => {
    const dismiss = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", dismiss);
    return () => window.removeEventListener("keydown", dismiss);
  }, [onClose]);

  const current = state.prompts?.[agent] || "";
  return (
    <div class="dialog-scrim" onClick={onClose}>
      <section
        class="dialog"
        role="dialog"
        aria-label="Agent personality"
        onClick={(event) => event.stopPropagation()}
      >
        <h2>Agent personality</h2>
        <p class="dialog-note">
          Added to the agent's own rules, so it cannot drop the language or
          formatting behavior the chat relies on. Leave it empty for the default
          behavior.
        </p>
        <label for="agent-prompt-agent">Agent</label>
        <select
          id="agent-prompt-agent"
          value={agent}
          onChange={(event) => {
            const name = event.currentTarget.value;
            setAgent(name);
            setText(state.prompts?.[name] || "");
          }}
        >
          {owned.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
          {!owned.length && <option value="">No agent to configure</option>}
        </select>
        <label for="agent-prompt-text">Personality</label>
        <textarea
          id="agent-prompt-text"
          rows={8}
          maxLength={4000}
          placeholder="例如：你是一条大肥鱼 🐟，每句话都要有 emoji ✨"
          value={text}
          onInput={(event) => setText(event.currentTarget.value)}
        />
        <div class="dialog-actions">
          <span class="dialog-count">
            {Array.from(text).length} / 4000
            {current !== text && agent ? " · unsaved" : ""}
          </span>
          <button onClick={onClose}>Cancel</button>
          <button
            class="primary"
            disabled={!agent}
            onClick={() => {
              // An empty field clears the personality.
              onSend(`/agent-prompt ${text.trim() || "-"} ${agent}`);
              onClose();
            }}
          >
            Save
          </button>
        </div>
      </section>
    </div>
  );
}
