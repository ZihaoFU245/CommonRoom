import { useEffect, useRef } from "preact/hooks";
import type { Fonts } from "../api/protocol.ts";
import { defaultFonts } from "../features/preferences/settings.ts";

export function Settings({
  fonts,
  onChange,
  onClose,
}: {
  fonts: Fonts;
  onChange: (value: Fonts) => void;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    element.showModal();
    return () => element.close();
  }, []);
  return (
    <dialog
      class="settings-dialog"
      ref={dialog}
      aria-labelledby="settings-title"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      <header class="settings-heading">
        <h2 id="settings-title">Settings</h2>
        <button
          class="settings-close"
          aria-label="Close settings"
          onClick={onClose}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="m6 6 12 12M18 6 6 18" />
          </svg>
        </button>
      </header>
      <div class="font-setting">
        <label for="chat-font-size">Chat font size</label>
        <output for="chat-font-size">{fonts.chat}px</output>
        <input
          id="chat-font-size"
          type="range"
          min="14"
          max="24"
          step="1"
          value={fonts.chat}
          onInput={(event) =>
            onChange({ ...fonts, chat: Number(event.currentTarget.value) })
          }
        />
      </div>
      <div class="font-setting">
        <label for="ui-font-size">UI font size</label>
        <output for="ui-font-size">{fonts.ui}px</output>
        <input
          id="ui-font-size"
          type="range"
          min="12"
          max="18"
          step="1"
          value={fonts.ui}
          onInput={(event) =>
            onChange({ ...fonts, ui: Number(event.currentTarget.value) })
          }
        />
      </div>
      <p class="font-preview">The quick brown fox · 你好 · مرحبا 👋</p>
      <button
        class="settings-reset"
        onClick={() => onChange({ ...defaultFonts })}
      >
        Reset defaults
      </button>
    </dialog>
  );
}
