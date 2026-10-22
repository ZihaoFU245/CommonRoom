import type { SettingsProps } from "./types.ts";

export function FormattingSettings({
  markdown,
  onMarkdownChange,
  latex,
  onLatexChange,
}: Pick<
  SettingsProps,
  "markdown" | "onMarkdownChange" | "latex" | "onLatexChange"
>) {
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
    </>
  );
}
