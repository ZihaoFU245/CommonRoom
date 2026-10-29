import { useExternalAvatars } from "../../hooks/useExternalAvatars.ts";
import { AccountEditor } from "./AccountEditor.tsx";
import type { SettingsProps } from "./types.ts";

export function SecuritySettings(
  props: Pick<
    SettingsProps,
    "state" | "pending" | "onAccountCommand" | "allowHtml" | "onAllowHtmlChange"
  >,
) {
  const { allowHtml, onAllowHtmlChange } = props;
  const [externalAvatars, setExternalAvatars] = useExternalAvatars();
  return (
    <>
      <AccountEditor
        state={props.state}
        pending={props.pending}
        onAccountCommand={props.onAccountCommand}
        section="security"
      />
      <>
        {" "}
        <h3>Message content</h3>
        <label class="security-choice">
          <input
            type="checkbox"
            checked={allowHtml}
            aria-describedby="markdown-html-description"
            onChange={(event) => onAllowHtmlChange(event.currentTarget.checked)}
          />
          <span>Allow HTML</span>
        </label>
        <p id="markdown-html-description" class="account-summary">
          Offer an isolated preview for all HTML, CSS and JavaScript. Each
          message requires a click on Render HTML before it runs. Render only
          messages you trust: previews can run scripts and show misleading
          content.
        </p>
        <p class="account-summary">
          External message images load only after you click Load image. Their
          hosts receive your IP address when you choose to load them.
        </p>
        <h3>Avatars</h3>
        <label class="security-choice">
          <input
            type="checkbox"
            checked={externalAvatars}
            aria-describedby="external-avatars-description"
            onChange={(event) =>
              setExternalAvatars(event.currentTarget.checked)
            }
          />
          <span>Load external avatars</span>
        </label>
        <p id="external-avatars-description" class="account-summary">
          External image hosts receive your IP address and may serve large
          images. Managed uploads load without this option.
        </p>
      </>
    </>
  );
}
