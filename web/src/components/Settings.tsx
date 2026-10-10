import { useEffect, useRef, useState } from "preact/hooks";
import {
  errorMessage,
  type Acknowledgement,
  type Snapshot,
} from "../api/protocol.ts";
import type { Fonts } from "../api/protocol.ts";
import { defaultSeeds, parseHex } from "../features/preferences/palette.ts";
import {
  defaultFonts,
  defaultTheme,
  type Seeds,
  type Theme,
} from "../features/preferences/settings.ts";

const themes: { value: Theme; label: string }[] = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" },
  { value: "custom", label: "Custom" },
];

function SeedField({
  id,
  label,
  value,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const parsed = parseHex(value);
  return (
    <div class="palette-seed">
      <label for={id}>{label}</label>
      <input
        class="palette-swatch"
        type="color"
        aria-label={`${label} color picker`}
        value={parsed ?? defaultSeeds.base}
        onInput={(event) => onChange(event.currentTarget.value)}
      />
      <input
        id={id}
        class="palette-hex"
        type="text"
        spellcheck={false}
        autoComplete="off"
        maxLength={7}
        value={value}
        aria-invalid={parsed === null}
        onInput={(event) => onChange(event.currentTarget.value)}
      />
    </div>
  );
}

export function Settings({
  state,
  pending,
  onAccountCommand,
  fonts,
  theme,
  seeds,
  onChange,
  onThemeChange,
  onSeedsChange,
  onClose,
}: {
  state: Snapshot;
  pending: boolean;
  onAccountCommand: (text: string) => Promise<Acknowledgement>;
  fonts: Fonts;
  theme: Theme;
  seeds: Seeds;
  onChange: (value: Fonts) => void;
  onThemeChange: (value: Theme) => void;
  onSeedsChange: (value: Seeds) => void;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<"account" | "appearance">("account");
  const [name, setName] = useState(state.username);
  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [feedback, setFeedback] = useState("");
  const [failed, setFailed] = useState(false);
  const canCommand = (command: string) =>
    state.account_access.commands.some((item) => item.name === command);
  useEffect(() => setName(state.username), [state.username]);
  function submit(text: string, password = false) {
    setFeedback("");
    onAccountCommand(text)
      .then((result) => {
        setFailed(result.kind === "error");
        setFeedback(result.text);
        if (password) {
          setOldPassword("");
          setNewPassword("");
          setConfirmation("");
        }
      })
      .catch((error: unknown) => {
        setFailed(true);
        setFeedback(errorMessage(error));
      });
  }
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
          <button
            type="button"
            aria-current={tab === "account" ? "page" : undefined}
            onClick={() => setTab("account")}
          >
            Account
          </button>
          <button
            type="button"
            aria-current={tab === "appearance" ? "page" : undefined}
            onClick={() => setTab("appearance")}
          >
            Appearance
          </button>
        </nav>
        <section
          key={tab}
          class="settings-content"
          tabIndex={0}
          aria-label={tab === "account" ? "Account" : "Appearance"}
        >
          {tab === "account" ? (
            <>
              <h3>Account</h3>
              <p class="account-summary">
                Signed in as <strong>{state.username}</strong> ·{" "}
                {state.groups.join(", ")}
              </p>
              <form
                class="account-form"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (!name || /\s/u.test(name)) {
                    setFailed(true);
                    setFeedback("Username cannot contain whitespace.");
                    return;
                  }
                  submit(`/rename ${name}`);
                }}
              >
                <label for="account-name">Username</label>
                <input
                  id="account-name"
                  autoComplete="username"
                  value={name}
                  required
                  onInput={(event) => setName(event.currentTarget.value)}
                />
                <button
                  type="submit"
                  disabled={
                    pending ||
                    !canCommand("/rename") ||
                    !state.account_access.permissions.some((permission) =>
                      ["w:account.rename.own", "x:account.rename.any"].includes(
                        permission,
                      ),
                    ) ||
                    name === state.username
                  }
                >
                  Rename
                </button>
              </form>
              <form
                class="account-form"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (newPassword !== confirmation) {
                    setFailed(true);
                    setFeedback("New passwords do not match.");
                    return;
                  }
                  if (/\s/u.test(oldPassword + newPassword)) {
                    setFailed(true);
                    setFeedback("Password commands cannot contain whitespace.");
                    return;
                  }
                  submit(`/passwd ${oldPassword} ${newPassword}`, true);
                }}
              >
                <h3>Change password</h3>
                <label for="account-old-password">Current password</label>
                <input
                  id="account-old-password"
                  type="password"
                  autoComplete="current-password"
                  required
                  value={oldPassword}
                  onInput={(event) => setOldPassword(event.currentTarget.value)}
                />
                <label for="account-new-password">New password</label>
                <input
                  id="account-new-password"
                  type="password"
                  autoComplete="new-password"
                  required
                  value={newPassword}
                  onInput={(event) => setNewPassword(event.currentTarget.value)}
                />
                <label for="account-confirm-password">
                  Confirm new password
                </label>
                <input
                  id="account-confirm-password"
                  type="password"
                  autoComplete="new-password"
                  required
                  value={confirmation}
                  onInput={(event) =>
                    setConfirmation(event.currentTarget.value)
                  }
                />
                <p class="account-summary">
                  Changing your password signs out your other sessions.
                </p>
                <button
                  type="submit"
                  disabled={
                    pending ||
                    !canCommand("/passwd") ||
                    !state.account_access.permissions.includes(
                      "w:account.password.own",
                    )
                  }
                >
                  Change password
                </button>
              </form>
              {feedback && (
                <p
                  class={failed ? "account-error" : "account-feedback"}
                  role="status"
                >
                  {feedback}
                </p>
              )}
            </>
          ) : (
            <>
              <fieldset class="theme-setting">
                <legend>Theme</legend>
                <div class="theme-choices">
                  {themes.map((choice) => (
                    <label class="theme-choice" key={choice.value}>
                      <input
                        type="radio"
                        name="theme"
                        value={choice.value}
                        checked={theme === choice.value}
                        onChange={() => onThemeChange(choice.value)}
                      />
                      <span>{choice.label}</span>
                    </label>
                  ))}
                </div>
              </fieldset>
              {theme === "custom" && (
                <div class="palette-seeds">
                  <SeedField
                    id="palette-base"
                    label="Surface"
                    value={seeds.base}
                    onChange={(base) => onSeedsChange({ ...seeds, base })}
                  />
                  <SeedField
                    id="palette-accent"
                    label="Accent"
                    value={seeds.accent}
                    onChange={(accent) => onSeedsChange({ ...seeds, accent })}
                  />
                  <hr class="palette-divider" />
                </div>
              )}
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
                    onChange({
                      ...fonts,
                      chat: Number(event.currentTarget.value),
                    })
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
                    onChange({
                      ...fonts,
                      ui: Number(event.currentTarget.value),
                    })
                  }
                />
              </div>
              <button
                class="settings-reset"
                onClick={() => {
                  onChange({ ...defaultFonts });
                  onThemeChange(defaultTheme);
                  onSeedsChange({ ...defaultSeeds });
                }}
              >
                Reset defaults
              </button>
            </>
          )}
        </section>
      </div>
    </dialog>
  );
}
