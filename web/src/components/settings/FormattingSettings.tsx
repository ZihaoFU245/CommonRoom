import { useSettingsMotion } from "../../hooks/useSettingsMotion.ts";
import { lazy, Suspense } from "preact/compat";
const NameColorSettings = lazy(() =>
  import("./NameColorSettings.tsx").then((module) => ({
    default: module.NameColorSettings,
  })),
);
import type { SettingsProps } from "./types.ts";

export function FormattingSettings({
  state,
  nameColors,
  onNameColorsChange,
  onNameColorChange,
  markdown,
  onMarkdownChange,
  latex,
  onLatexChange,
  messageBars,
  onMessageBarsChange,
  messageBarOpacity,
  onMessageBarOpacityChange,
  nameBorders,
  onNameBordersChange,
}: Pick<
  SettingsProps,
  | "state"
  | "nameColors"
  | "onNameColorsChange"
  | "onNameColorChange"
  | "markdown"
  | "onMarkdownChange"
  | "latex"
  | "onLatexChange"
  | "messageBars"
  | "onMessageBarsChange"
  | "messageBarOpacity"
  | "onMessageBarOpacityChange"
  | "nameBorders"
  | "onNameBordersChange"
>) {
  const content = useSettingsMotion<HTMLDivElement>();
  return (
    <div class="formatting-settings" ref={content}>
      <h3>Preferences</h3>
      <label class="security-choice">
        <input
          type="checkbox"
          checked={markdown}
          aria-describedby="markdown-preference-description"
          onChange={(event) => onMarkdownChange(event.currentTarget.checked)}
        />
        <span>Enable Markdown</span>
      </label>
      <p id="markdown-preference-description" class="account-summary">
        Format new messages with Markdown.
      </p>
      <label class="security-choice">
        <input
          type="checkbox"
          checked={latex}
          aria-describedby="latex-preference-description"
          onChange={(event) => onLatexChange(event.currentTarget.checked)}
        />
        <span>Enable LaTeX math</span>
      </label>
      <p id="latex-preference-description" class="account-summary">
        Render equations in Markdown messages.
      </p>
      <label class="security-choice">
        <input
          type="checkbox"
          checked={nameColors}
          aria-describedby="name-colors-preference-description"
          onChange={(event) => onNameColorsChange(event.currentTarget.checked)}
        />
        <span>Show name colors</span>
      </label>
      <p id="name-colors-preference-description" class="account-summary">
        Show each person's chosen name color. Saved on this device.
      </p>
      {nameColors && (
        <div class="name-color-controls">
          <Suspense
            fallback={<p class="account-summary">Loading name colors…</p>}
          >
            <NameColorSettings
              state={state}
              onNameColorChange={onNameColorChange}
            />
          </Suspense>
        </div>
      )}
      <label class="security-choice">
        <input
          type="checkbox"
          checked={messageBars}
          aria-describedby="message-bars-preference-description"
          onChange={(event) => onMessageBarsChange(event.currentTarget.checked)}
        />
        <span>Show message color bars</span>
      </label>
      <p id="message-bars-preference-description" class="account-summary">
        Mark each message with its author's name color. Saved on this device.
      </p>
      {messageBars && (
        <div class="font-setting message-bar-opacity-setting">
          <label for="message-bar-opacity">Bar opacity</label>
          <output for="message-bar-opacity">{messageBarOpacity}%</output>
          <input
            id="message-bar-opacity"
            type="range"
            min="0"
            max="100"
            step="1"
            value={messageBarOpacity}
            aria-valuetext={`${messageBarOpacity}%`}
            aria-describedby="message-bar-opacity-description"
            onInput={(event) =>
              onMessageBarOpacityChange(event.currentTarget.valueAsNumber)
            }
          />
          <p id="message-bar-opacity-description" class="account-summary">
            Fade the message bar without changing names or reply quotes. Saved
            on this device.
          </p>
        </div>
      )}
      <label class="security-choice">
        <input
          type="checkbox"
          checked={nameBorders}
          aria-describedby="name-borders-preference-description"
          onChange={(event) => onNameBordersChange(event.currentTarget.checked)}
        />
        <span>Show name borders</span>
      </label>
      <p id="name-borders-preference-description" class="account-summary">
        Frame author names in a rounded badge. Saved on this device.
      </p>
    </div>
  );
}
