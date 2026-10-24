import { controlMotion } from "../../features/preferences/motion.ts";
import autoAnimate from "@formkit/auto-animate";
import { useEffect, useRef, useState } from "preact/hooks";
import { errorMessage } from "../../api/protocol.ts";
import type { SettingsProps } from "./types.ts";

export function AccountEditor({
  state,
  pending,
  onAccountCommand,
  section,
}: Pick<SettingsProps, "state" | "pending" | "onAccountCommand"> & {
  section: "account" | "security";
}) {
  useEffect(
    () => () => {
      operation.current += 1;
    },
    [],
  );
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
  useEffect(() => {
    if (editor) editorInput.current?.focus();
    else if (restoreEditorFocus.current) editorButton.current?.focus();
    restoreEditorFocus.current = false;
  }, [editor]);
  useEffect(() => {
    if (!content.current || (section !== "account" && section !== "security"))
      return;
    const animation = autoAnimate(content.current, controlMotion);
    return () => animation.destroy?.();
  }, [section]);
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
  return (
    <div ref={content}>
      {section === "account" ? (
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
                  disabled={pending || !canRename || name === state.username}
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
      ) : (
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
              disabled={pending || !canChangePassword || editor === "password"}
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
                  setFeedback("Password commands cannot contain whitespace.");
                  return;
                }
                submit(`/user passwd ${oldPassword} ${newPassword}`, true);
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
                onInput={(event) => setOldPassword(event.currentTarget.value)}
              />
              <label for="account-new-password">New password</label>
              <input
                id="account-new-password"
                type="password"
                disabled={pending}
                autoComplete="new-password"
                required
                value={newPassword}
                onInput={(event) => setNewPassword(event.currentTarget.value)}
              />
              <label for="account-confirm-password">Confirm new password</label>
              <input
                id="account-confirm-password"
                type="password"
                disabled={pending}
                autoComplete="new-password"
                required
                value={confirmation}
                onInput={(event) => setConfirmation(event.currentTarget.value)}
              />
              <p class="account-summary">
                Changing your password signs out your other sessions.
              </p>
              <div class="account-editor-actions">
                <button type="submit" disabled={pending || !canChangePassword}>
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
        </>
      )}
    </div>
  );
}
