import { useRef, useState } from "preact/hooks";
import { errorMessage } from "../api/protocol.ts";
import type { Aliases } from "../features/conversation/aliases.ts";
import { redactCommand } from "../features/conversation/console.ts";

export function AliasSettings({
  aliases,
  onSave,
  onRemove,
}: {
  aliases: Aliases;
  onSave: (name: string, target: string) => void;
  onRemove: (name: string) => void;
}) {
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [feedback, setFeedback] = useState("");
  const [failed, setFailed] = useState(false);
  const nameInput = useRef<HTMLInputElement>(null);
  const mappings = Object.entries(aliases).sort(([a], [b]) =>
    a.localeCompare(b),
  );
  return (
    <>
      <h3>Alias</h3>
      <p class="account-summary">
        Map a command name to another command, such as /rooms to /ls room. Saved
        in this browser and shared by accounts using it.
      </p>
      <form
        class="account-form"
        onSubmit={(event) => {
          event.preventDefault();
          try {
            onSave(name, target);
            setFailed(false);
            setFeedback("Alias saved.");
            setName("");
            setTarget("");
          } catch (error: unknown) {
            setFailed(true);
            setFeedback(errorMessage(error));
          }
        }}
      >
        <label for="alias-name">Command name</label>
        <input
          id="alias-name"
          ref={nameInput}
          value={name}
          placeholder="/rooms"
          autoComplete="off"
          spellcheck={false}
          required
          onInput={(event) => setName(event.currentTarget.value)}
        />
        <label for="alias-target">Expands to</label>
        <input
          id="alias-target"
          value={target}
          placeholder="/ls room"
          autoComplete="off"
          spellcheck={false}
          required
          onInput={(event) => setTarget(event.currentTarget.value)}
        />
        <p class="account-summary">
          Saving an existing name replaces its mapping. /alias, /sudo and
          /markdown cannot be remapped.
        </p>
        <button type="submit">Save alias</button>
      </form>
      {feedback && (
        <p
          class={`alias-feedback ${failed ? "account-error" : "account-feedback"}`}
          role="status"
        >
          {feedback}
        </p>
      )}
      <h3>Saved aliases</h3>
      {mappings.length ? (
        <ul class="alias-list" aria-label="Saved aliases">
          {mappings.map(([alias, expansion]) => (
            <li key={alias}>
              <div class="alias-mapping">
                <code>{alias}</code>
                <span>→</span>
                <code>{redactCommand(expansion)}</code>
              </div>
              <div class="alias-actions">
                <button
                  type="button"
                  aria-label={`Edit ${alias}`}
                  onClick={() => {
                    setName(alias);
                    setTarget(expansion);
                    setFeedback("");
                    nameInput.current?.focus();
                  }}
                >
                  Edit
                </button>
                <button
                  type="button"
                  aria-label={`Remove ${alias}`}
                  onClick={() => {
                    try {
                      onRemove(alias);
                      if (name.trim() === alias) {
                        setName("");
                        setTarget("");
                      }
                      setFailed(false);
                      setFeedback(`Removed ${alias}.`);
                    } catch (error: unknown) {
                      setFailed(true);
                      setFeedback(errorMessage(error));
                    }
                  }}
                >
                  Remove
                </button>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <p class="account-summary">No aliases saved.</p>
      )}
    </>
  );
}
