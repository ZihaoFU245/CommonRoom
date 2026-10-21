import autoAnimate from "@formkit/auto-animate";
import { useExternalAvatars } from "../hooks/useExternalAvatars.ts";
import { AvatarSettings } from "./AvatarSettings.tsx";
import { useEffect, useRef, useState } from "preact/hooks";
import { AliasSettings } from "./AliasSettings.tsx";
import type { Aliases } from "../features/conversation/aliases.ts";
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
  aliases,
  onSaveAlias,
  onRemoveAlias,
  fonts,
  theme,
  seeds,
  markdown,
  onMarkdownChange,
  latex,
  onLatexChange,
  allowHtml,
  onAllowHtmlChange,
  onChange,
  onThemeChange,
  onSeedsChange,
  onClose,
}: {
  state: Snapshot;
  pending: boolean;
  onAccountCommand: (text: string) => Promise<Acknowledgement>;
  aliases: Aliases;
  onSaveAlias: (name: string, target: string) => void;
  onRemoveAlias: (name: string) => void;
  fonts: Fonts;
  theme: Theme;
  seeds: Seeds;
  markdown: boolean;
  onMarkdownChange: (enabled: boolean) => void;
  latex: boolean;
  onLatexChange: (enabled: boolean) => void;
  allowHtml: boolean;
  onAllowHtmlChange: (enabled: boolean) => void;
  onChange: (value: Fonts) => void;
  onThemeChange: (value: Theme) => void;
  onSeedsChange: (value: Seeds) => void;
  onClose: () => void;
}) {
  const [externalAvatars, setExternalAvatars] = useExternalAvatars();
  const [tab, setTab] = useState<
    "account" | "appearance" | "alias" | "security" | "preferences"
  >("account");
  const [editor, setEditor] = useState<"rename" | "password" | null>(null);
  const editorInput = useRef<HTMLInputElement>(null);
  const editorButton = useRef<HTMLButtonElement>(null);
  const restoreEditorFocus = useRef(false);
  const content = useRef<HTMLDivElement>(null);
  const operation = useRef(0);
  const [name, setName] = useState(state.username);
  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [feedback, setFeedback] = useState("");
  const [failed, setFailed] = useState(false);
  const canCommand = (command: string) =>
    state.account_access.commands.some((item) => item.name === command);
  useEffect(() => setName(state.username), [state.username]);
  const canRename =
    canCommand("/user") &&
    state.account_access.permissions.some((permission) =>
      ["w:account.rename.own", "x:account.rename.any"].includes(permission),
    );
  const canChangePassword =
    canCommand("/user") &&
    state.account_access.permissions.includes("w:account.password.own");
  function clearPassword() {
    setOldPassword("");
    setNewPassword("");
    setConfirmation("");
  }
  function closeEditor() {
    operation.current += 1;
    restoreEditorFocus.current = true;
    setEditor(null);
    setName(state.username);
    clearPassword();
    setFeedback("");
  }
  function selectTab(next: typeof tab) {
    closeEditor();
    restoreEditorFocus.current = false;
    setTab(next);
  }
  useEffect(() => {
    if (editor) editorInput.current?.focus();
    else if (restoreEditorFocus.current) editorButton.current?.focus();
    restoreEditorFocus.current = false;
  }, [editor]);
  useEffect(() => {
    if (!content.current || (tab !== "account" && tab !== "security")) return;
    const animation = autoAnimate(content.current, {
      duration: 120,
      easing: "ease-out",
    });
    return () => animation.destroy?.();
  }, [tab]);
  function submit(text: string, password = false) {
    const currentOperation = ++operation.current;
    setFeedback("");
    onAccountCommand(text)
      .then((result) => {
        if (currentOperation !== operation.current) return;
        setFailed(result.kind === "error");
        setFeedback(result.text);
        if (password) clearPassword();
        if (result.kind !== "error") {
          restoreEditorFocus.current = true;
          setEditor(null);
        }
      })
      .catch((error: unknown) => {
        if (currentOperation !== operation.current) return;
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
            onClick={() => selectTab("account")}
          >
            Account
          </button>
          <button
            type="button"
            aria-current={tab === "appearance" ? "page" : undefined}
            onClick={() => selectTab("appearance")}
          >
            Appearance
          </button>
          <button
            type="button"
            aria-current={tab === "alias" ? "page" : undefined}
            onClick={() => selectTab("alias")}
          >
            Alias
          </button>
          <button
            type="button"
            aria-current={tab === "preferences" ? "page" : undefined}
            onClick={() => selectTab("preferences")}
          >
            Preferences
          </button>
          <button
            type="button"
            aria-current={tab === "security" ? "page" : undefined}
            onClick={() => selectTab("security")}
          >
            Security
          </button>
        </nav>
        <section
          class="settings-content"
          tabIndex={0}
          aria-label={
            tab === "account"
              ? "Account"
              : tab === "appearance"
                ? "Appearance"
                : tab === "alias"
                  ? "Alias"
                  : tab === "security"
                    ? "Security"
                    : "Preferences"
          }
        >
          <div hidden={tab !== "account"}>
            <h3>Account</h3>
            <AvatarSettings state={state} />
          </div>
          <div key={tab} ref={content}>
            {tab === "account" ? (
              <>
                <div class="account-setting-row">
                  <div class="account-setting-detail">
                    <strong>Username</strong>
                    <p>{state.username}</p>
                  </div>
                  <button
                    type="button"
                    ref={editorButton}
                    class="account-setting-action"
                    aria-expanded={editor === "rename"}
                    aria-controls="account-rename-editor"
                    disabled={pending || !canRename || editor === "rename"}
                    onClick={() => {
                      setFeedback("");
                      setEditor("rename");
                    }}
                  >
                    Edit
                  </button>
                </div>
                {editor === "rename" && (
                  <form
                    class="account-form account-editor"
                    id="account-rename-editor"
                    onSubmit={(event) => {
                      event.preventDefault();
                      if (!name || /\s/u.test(name)) {
                        setFailed(true);
                        setFeedback("Username cannot contain whitespace.");
                        return;
                      }
                      submit(`/user rename ${name}`);
                    }}
                  >
                    <label for="account-name">Username</label>
                    <input
                      id="account-name"
                      ref={editorInput}
                      disabled={pending}
                      autoComplete="username"
                      value={name}
                      required
                      onInput={(event) => setName(event.currentTarget.value)}
                    />
                    <div class="account-editor-actions">
                      <button
                        type="submit"
                        disabled={
                          pending || !canRename || name === state.username
                        }
                      >
                        Save
                      </button>
                      <button
                        type="button"
                        class="account-cancel"
                        disabled={pending}
                        onClick={closeEditor}
                      >
                        Cancel
                      </button>
                    </div>
                  </form>
                )}
                {feedback && (
                  <p
                    class={failed ? "account-error" : "account-feedback"}
                    role="status"
                  >
                    {feedback}
                  </p>
                )}
              </>
            ) : tab === "preferences" ? (
              <>
                <h3>Preferences</h3>
                <label class="security-choice">
                  <input
                    type="checkbox"
                    checked={markdown}
                    aria-describedby="markdown-preference-description"
                    onChange={(event) =>
                      onMarkdownChange(event.currentTarget.checked)
                    }
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
                    onChange={(event) =>
                      onLatexChange(event.currentTarget.checked)
                    }
                  />
                  <span>Enable LaTeX math</span>
                </label>
                <p id="latex-preference-description" class="account-summary">
                  Render equations in Markdown messages.
                </p>
              </>
            ) : tab === "security" ? (
              <>
                <h3>Security</h3>
                <div class="account-setting-row">
                  <div class="account-setting-detail">
                    <strong>Password</strong>
                    <p>Update your account password.</p>
                  </div>
                  <button
                    type="button"
                    ref={editorButton}
                    class="account-setting-action"
                    aria-expanded={editor === "password"}
                    aria-controls="account-password-editor"
                    disabled={
                      pending || !canChangePassword || editor === "password"
                    }
                    onClick={() => {
                      setFeedback("");
                      setEditor("password");
                    }}
                  >
                    Change
                  </button>
                </div>
                {editor === "password" && (
                  <form
                    class="account-form account-editor"
                    id="account-password-editor"
                    onSubmit={(event) => {
                      event.preventDefault();
                      if (newPassword !== confirmation) {
                        setFailed(true);
                        setFeedback("New passwords do not match.");
                        return;
                      }
                      if (/\s/u.test(oldPassword + newPassword)) {
                        setFailed(true);
                        setFeedback(
                          "Password commands cannot contain whitespace.",
                        );
                        return;
                      }
                      submit(
                        `/user passwd ${oldPassword} ${newPassword}`,
                        true,
                      );
                    }}
                  >
                    <label for="account-old-password">Current password</label>
                    <input
                      id="account-old-password"
                      ref={editorInput}
                      type="password"
                      disabled={pending}
                      autoComplete="current-password"
                      required
                      value={oldPassword}
                      onInput={(event) =>
                        setOldPassword(event.currentTarget.value)
                      }
                    />
                    <label for="account-new-password">New password</label>
                    <input
                      id="account-new-password"
                      type="password"
                      disabled={pending}
                      autoComplete="new-password"
                      required
                      value={newPassword}
                      onInput={(event) =>
                        setNewPassword(event.currentTarget.value)
                      }
                    />
                    <label for="account-confirm-password">
                      Confirm new password
                    </label>
                    <input
                      id="account-confirm-password"
                      type="password"
                      disabled={pending}
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
                    <div class="account-editor-actions">
                      <button
                        type="submit"
                        disabled={pending || !canChangePassword}
                      >
                        Save password
                      </button>
                      <button
                        type="button"
                        class="account-cancel"
                        disabled={pending}
                        onClick={closeEditor}
                      >
                        Cancel
                      </button>
                    </div>
                  </form>
                )}
                {feedback && (
                  <p
                    class={failed ? "account-error" : "account-feedback"}
                    role="status"
                  >
                    {feedback}
                  </p>
                )}
                <h3>Message content</h3>
                <label class="security-choice">
                  <input
                    type="checkbox"
                    checked={allowHtml}
                    aria-describedby="markdown-html-description"
                    onChange={(event) =>
                      onAllowHtmlChange(event.currentTarget.checked)
                    }
                  />
                  <span>Allow HTML</span>
                </label>
                <p id="markdown-html-description" class="account-summary">
                  Offer an isolated preview for all HTML, CSS and JavaScript.
                  Each message requires a click on Render HTML before it runs.
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
                  External image hosts receive your IP address and may serve
                  large images. Managed uploads load without this option.
                </p>
              </>
            ) : tab === "alias" ? (
              <AliasSettings
                aliases={aliases}
                onSave={onSaveAlias}
                onRemove={onRemoveAlias}
              />
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
          </div>
        </section>
      </div>
    </dialog>
  );
}
