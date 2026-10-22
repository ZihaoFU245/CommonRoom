import { useEffect, useRef, useState } from "preact/hooks";
import { AvatarSettings } from "./AvatarSettings.tsx";
import { AliasSettings } from "./AliasSettings.tsx";
import { AccountEditor } from "./settings/AccountEditor.tsx";
import { AppearanceSettings } from "./settings/AppearanceSettings.tsx";
import { FormattingSettings } from "./settings/FormattingSettings.tsx";
import { SecuritySettings } from "./settings/SecuritySettings.tsx";
import type { SettingsProps } from "./settings/types.ts";

const tabs = {
  account: "Account",
  appearance: "Appearance",
  alias: "Alias",
  preferences: "Preferences",
  security: "Security",
} as const;
type Tab = keyof typeof tabs;
export function Settings(props: SettingsProps) {
  const { state, onClose } = props;
  const [tab, setTab] = useState<Tab>("account");
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
      <div class="settings-layout">
        <nav class="settings-navigation" aria-label="Settings sections">
          {(Object.keys(tabs) as Tab[]).map((value) => (
            <button
              key={value}
              type="button"
              aria-current={tab === value ? "page" : undefined}
              onClick={() => setTab(value)}
            >
              {tabs[value]}
            </button>
          ))}
        </nav>
        <section class="settings-content" tabIndex={0} aria-label={tabs[tab]}>
          <div hidden={tab !== "account"}>
            <h3>Account</h3>
            <AvatarSettings state={state} />
          </div>
          {tab === "account" ? (
            <AccountEditor
              key="account"
              state={state}
              pending={props.pending}
              onAccountCommand={props.onAccountCommand}
              section="account"
            />
          ) : tab === "security" ? (
            <SecuritySettings {...props} />
          ) : tab === "preferences" ? (
            <FormattingSettings {...props} />
          ) : tab === "alias" ? (
            <AliasSettings
              aliases={props.aliases}
              onSave={props.onSaveAlias}
              onRemove={props.onRemoveAlias}
            />
          ) : (
            <AppearanceSettings {...props} />
          )}
        </section>
      </div>
    </dialog>
  );
}
