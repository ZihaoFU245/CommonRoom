import { useMessagePreviewHeight } from "../../hooks/useMessagePreviewHeight.ts";
import {
  messagePreviewMin,
  messagePreviewMax,
  messagePreviewStep,
} from "../../features/preferences/messagePreview.ts";
import type { SettingsProps } from "./types.ts";

export function FormattingSettings({
  nameColors,
  onNameColorsChange,
  markdown,
  onMarkdownChange,
  latex,
  onLatexChange,
  messageBars,
  onMessageBarsChange,
  nameBorders,
  onNameBordersChange,
}: Pick<
  SettingsProps,
  | "nameColors"
  | "onNameColorsChange"
  | "markdown"
  | "onMarkdownChange"
  | "latex"
  | "onLatexChange"
  | "messageBars"
  | "onMessageBarsChange"
  | "nameBorders"
  | "onNameBordersChange"
>) {
  const [previewHeight, setPreviewHeight] = useMessagePreviewHeight();
  const previewLabel =
    previewHeight === messagePreviewMax
      ? "Show full messages"
      : `${previewHeight}px`;
  return (
    <>
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
        Show each person's chosen name color. Choose your color in Appearance.
      </p>
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
        Mark each message with its author's name color. Adjust bar opacity in
        Appearance.
      </p>
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
        Frame author names in a rounded badge. Synced across your devices.
      </p>
      <div class="font-setting">
        <label for="message-preview-height">Message preview height</label>
        <output for="message-preview-height">{previewLabel}</output>
        <input
          id="message-preview-height"
          type="range"
          min={messagePreviewMin}
          max={messagePreviewMax}
          step={messagePreviewStep}
          value={previewHeight}
          aria-valuetext={previewLabel}
          onInput={(event) =>
            setPreviewHeight(event.currentTarget.valueAsNumber)
          }
        />
      </div>
    </>
  );
}
