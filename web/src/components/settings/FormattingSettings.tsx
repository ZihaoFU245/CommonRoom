import { useState } from "preact/hooks";
import {
  defaultPageTitle,
  loadPageTitle,
  savePageTitle,
} from "../../features/preferences/settings.ts";
import { useMessagePreviewHeight } from "../../hooks/useMessagePreviewHeight.ts";
import {
  messagePreviewMin,
  messagePreviewMax,
  messagePreviewStep,
} from "../../features/preferences/messagePreview.ts";
import type { SettingsProps } from "./types.ts";

export function FormattingSettings({
  alwaysSendWithEnter,
  onAlwaysSendWithEnterChange,
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
  showBotStatus,
  onShowBotStatusChange,
}: Pick<
  SettingsProps,
  | "alwaysSendWithEnter"
  | "onAlwaysSendWithEnterChange"
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
  | "showBotStatus"
  | "onShowBotStatusChange"
>) {
  const [previewHeight, setPreviewHeight] = useMessagePreviewHeight();
  const [pageTitle, setPageTitle] = useState(loadPageTitle);
  const [titleFeedback, setTitleFeedback] = useState("");
  const previewLabel =
    previewHeight === messagePreviewMax
      ? "Show full messages"
      : `${previewHeight}px`;
  return (
    <>
      <h3>Preferences</h3>
      <form
        class="account-form"
        onSubmit={(event) => {
          event.preventDefault();
          const value = new FormData(event.currentTarget).get("page-title");
          const title = typeof value === "string" ? value.trim() : "";
          if (!savePageTitle(title)) {
            setTitleFeedback("Could not save the page title in this browser.");
            return;
          }
          document.title = title || defaultPageTitle;
          setPageTitle(title);
          setTitleFeedback("Page title saved.");
        }}
      >
        <label for="page-title">Page title</label>
        <input
          id="page-title"
          name="page-title"
          type="text"
          placeholder={defaultPageTitle}
          aria-describedby="page-title-description"
          value={pageTitle}
          onInput={(event) => {
            setPageTitle(event.currentTarget.value);
            setTitleFeedback("");
          }}
        />
        <p id="page-title-description" class="account-summary">
          Applies in this browser. Leave blank to use CommonRoom.
        </p>
        <button type="submit">Save title</button>
        <span role="status">{titleFeedback}</span>
      </form>
      <label class="security-choice enter-send-choice" for="enter-send">
        <input
          id="enter-send"
          type="checkbox"
          checked={alwaysSendWithEnter}
          aria-describedby="enter-send-description"
          onChange={(event) =>
            onAlwaysSendWithEnterChange(event.currentTarget.checked)
          }
        />
        <span>Always send with Enter</span>
      </label>
      <p id="enter-send-description" class="account-summary">
        Applies only in this browser. Press Shift+Enter for a new line.
      </p>
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
      <label class="security-choice bot-status-choice">
        <input
          type="checkbox"
          checked={showBotStatus}
          aria-describedby="bot-status-preference-description"
          onChange={(event) =>
            onShowBotStatusChange(event.currentTarget.checked)
          }
        />
        <span>Show bot status</span>
      </label>
      <p id="bot-status-preference-description" class="account-summary">
        Show when bots are working and preview their drafts above the composer.
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
