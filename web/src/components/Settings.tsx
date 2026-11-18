import { useLayoutEffect, useRef, useState } from "preact/hooks";
import {
  focusControl,
  mobileLayout,
} from "../features/preferences/inputFocus.ts";
import {
  settingsSections,
  type SettingsScreen,
  type SettingsSection,
} from "../features/preferences/settingsRoute.ts";
import { AvatarSettings } from "./AvatarSettings.tsx";
import { AliasSettings } from "./AliasSettings.tsx";
import { AccountEditor } from "./settings/AccountEditor.tsx";
import { AppearanceSettings } from "./settings/AppearanceSettings.tsx";
import { FormattingSettings } from "./settings/FormattingSettings.tsx";
import { SecuritySettings } from "./settings/SecuritySettings.tsx";
import { StickerSettings } from "./settings/StickerSettings.tsx";
import type { SettingsProps } from "./settings/types.ts";

export function Settings(
  props: SettingsProps & {
    screen?: SettingsScreen | undefined;
    onSection?: (section: SettingsSection) => void;
    onBack?: () => void;
  },
) {
  const { state, screen, onSection, onBack, onClose } = props;
  const mobile = matchMedia(mobileLayout).matches;
  const section = mobile && screen !== "index" ? screen : undefined;
  const [desktopTab, setDesktopTab] = useState<SettingsSection>(
    screen && screen !== "index" ? screen : "account",
  );
  const tab = section ?? desktopTab;
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useRef<HTMLHeadingElement>(null);
  const previousSection = useRef<SettingsSection>();
  useLayoutEffect(() => {
    const element = dialog.current;
    if (!element) return;
    element.showModal();
    return () => element.close();
  }, []);
  useLayoutEffect(() => {
    const previous = previousSection.current;
    previousSection.current = section;
    if (!mobile || section === previous) return;
    if (section) focusControl(title.current, { preventScroll: true });
    else if (previous)
      focusControl(
        dialog.current?.querySelector<HTMLButtonElement>(
          `.settings-navigation [data-section="${previous}"]`,
        ),
        { preventScroll: true },
      );
  }, [mobile, section]);
  return (
    <dialog
      class={`settings-dialog ${section ? "settings-section-open" : ""}`}
      ref={dialog}
      aria-labelledby="settings-title"
      onCancel={(event) => {
        event.preventDefault();
        if (section && onBack) onBack();
        else onClose();
      }}
    >
      <header class="settings-heading">
        {section && (
          <button
            type="button"
            class="settings-back"
            aria-label="All settings"
            onClick={onBack}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="m14 6-6 6 6 6" />
            </svg>
          </button>
        )}
        <h2 id="settings-title" ref={title} tabIndex={-1}>
          <span class="settings-index-title">Settings</span>
          <span class="settings-section-title">{settingsSections[tab]}</span>
        </h2>
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
          {(Object.keys(settingsSections) as SettingsSection[]).map((value) => (
            <button
              key={value}
              type="button"
              data-section={value}
              aria-current={!mobile && tab === value ? "page" : undefined}
              onClick={() => {
                if (mobile) onSection?.(value);
                else setDesktopTab(value);
              }}
            >
              {settingsSections[value]}
            </button>
          ))}
        </nav>
        {(!mobile || section) && (
          <section
            class="settings-content"
            tabIndex={0}
            aria-label={settingsSections[tab]}
          >
            <div hidden={tab !== "account"}>
              {!mobile && <h3>Account</h3>}
              <AvatarSettings state={state} />
            </div>
            {tab === "account" ? (
              <>
                <AccountEditor
                  key="account"
                  state={state}
                  pending={props.pending}
                  onAccountCommand={props.onAccountCommand}
                  section="account"
                />
                <StickerSettings
                  key={state.username}
                  account={state.username}
                />
              </>
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
        )}
      </div>
    </dialog>
  );
}
