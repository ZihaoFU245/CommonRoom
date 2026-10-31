import { DisplayNameRecords } from "./DisplayNameRecords.tsx";
import { ScopePicker } from "./ScopePicker.tsx";
import { api } from "../../api/client.ts";
import { controlMotion } from "../../features/preferences/motion.ts";
import autoAnimate from "@formkit/auto-animate";
import { useEffect, useRef, useState } from "preact/hooks";
import { saveErrorMessage } from "../../features/preferences/saveFeedback.ts";
import type { SettingsProps } from "./types.ts";

function configuredName(
  names: Record<string, string> | undefined,
  username: string,
) {
  return names && Object.hasOwn(names, username) ? names[username] : undefined;
}

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
  const [editor, setEditor] = useState<
    "rename" | "displayname" | "password" | null
  >(null);
  const editorInput = useRef<HTMLInputElement>(null);
  const editorButton = useRef<HTMLButtonElement>(null);
  const restoreEditorFocus = useRef(false);
  const restoreDisplayNameFocus = useRef(false);
  const content = useRef<HTMLDivElement>(null);
  const operation = useRef(0);
  const [name, setName] = useState(state.username);
  const [displayNameScope, setDisplayNameScope] = useState<{
    id: string | null;
    label: string;
  }>({ id: null, label: "Everywhere" });
  const displayNameRooms = state.rooms.filter(
    (room) =>
      !room.name.startsWith("@") &&
      (room.permissions.includes("w:account.displayname.own") ||
        room.permissions.includes("w:account.displayname.any")),
  );
  const displayNameRoom = state.rooms.find(
    (room) => room.id === displayNameScope.id,
  );
  const displayNameScopeAvailable =
    displayNameScope.id === null ||
    displayNameRooms.some((room) => room.id === displayNameScope.id);
  const currentDisplayName =
    configuredName(
      displayNameScope.id === null
        ? state.display_names
        : displayNameRoom?.display_names,
      state.username,
    ) ?? "";
  const [displayName, setDisplayName] = useState(currentDisplayName);
  const displayNameEdited = useRef(false);
  const [savingDisplayName, setSavingDisplayName] = useState(false);
  const [removingDisplayName, setRemovingDisplayName] = useState<
    string | null | undefined
  >();
  const globalDisplayName = configuredName(state.display_names, state.username);
  const displayNameRecords = [
    ...(globalDisplayName
      ? [{ id: null, scope: "Global", name: globalDisplayName, editable: true }]
      : []),
    ...state.rooms.flatMap((room) => {
      const value = configuredName(room.display_names, state.username);
      return !room.name.startsWith("@") && value
        ? [
            {
              id: room.id,
              scope: `#${room.name}`,
              name: value,
              editable:
                room.permissions.includes("w:account.displayname.own") ||
                room.permissions.includes("w:account.displayname.any"),
            },
          ]
        : [];
    }),
  ];
  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [feedback, setFeedback] = useState("");
  const [failed, setFailed] = useState(false);
  const canCommand = (command: string) =>
    state.account_access.commands.some((item) => item.name === command);
  useEffect(() => setName(state.username), [state.username]);
  useEffect(() => {
    if (!displayNameEdited.current && displayNameScopeAvailable)
      setDisplayName(currentDisplayName);
  }, [currentDisplayName, displayNameScope.id, displayNameScopeAvailable]);
  useEffect(() => {
    if (!savingDisplayName && restoreDisplayNameFocus.current) {
      restoreDisplayNameFocus.current = false;
      editorInput.current?.focus({ preventScroll: true });
    }
  }, [savingDisplayName]);
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
    displayNameEdited.current = false;
    setDisplayName(currentDisplayName);
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
        setFeedback(
          result.kind === "error"
            ? saveErrorMessage(new Error(result.text))
            : result.text,
        );
        if (result.kind !== "error") {
          if (password) clearPassword();
          restoreEditorFocus.current = true;
          setEditor(null);
        }
      })
      .catch((error: unknown) => {
        if (currentOperation !== operation.current) return;
        setFailed(true);
        setFeedback(saveErrorMessage(error));
      });
  }
  function updateDisplayName(
    value: string | null,
    room: string | undefined,
    remove = false,
  ) {
    if (savingDisplayName || pending) return;
    const currentOperation = ++operation.current;
    setSavingDisplayName(true);
    setRemovingDisplayName(remove ? (room ?? null) : undefined);
    setFeedback("");
    api
      .setDisplayName(value, room)
      .then(() => {
        if (currentOperation !== operation.current) return;
        setFailed(false);
        setFeedback(remove ? "Display name removed." : "Display name saved.");
        if (remove) {
          restoreDisplayNameFocus.current = true;
          if (
            !displayNameEdited.current &&
            (displayNameScope.id ?? undefined) === room
          )
            setDisplayName("");
        } else {
          restoreEditorFocus.current = true;
          setEditor(null);
        }
      })
      .catch((error: unknown) => {
        if (currentOperation !== operation.current) return;
        setFailed(true);
        setFeedback(saveErrorMessage(error));
      })
      .finally(() => {
        if (currentOperation !== operation.current) return;
        setSavingDisplayName(false);
        setRemovingDisplayName(undefined);
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
              class="account-setting-action"
              aria-expanded={editor === "rename"}
              aria-controls="account-rename-editor"
              disabled={
                pending ||
                savingDisplayName ||
                !canRename ||
                editor === "rename"
              }
              onClick={(event) => {
                editorButton.current = event.currentTarget;
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
                  {pending ? "Saving…" : "Save"}
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
          <div class="account-setting-row displayname-setting-row">
            <div class="account-setting-detail displayname-setting-detail">
              <strong>Display name</strong>
              <DisplayNameRecords
                records={displayNameRecords}
                editing={editor === "displayname"}
                saving={pending || savingDisplayName}
                removingScope={removingDisplayName}
                onRemove={(id) =>
                  updateDisplayName(null, id ?? undefined, true)
                }
              />
            </div>
            <button
              type="button"
              class="account-setting-action"
              aria-expanded={editor === "displayname"}
              aria-controls="account-displayname-editor"
              disabled={
                pending || savingDisplayName || editor === "displayname"
              }
              onClick={(event) => {
                editorButton.current = event.currentTarget;
                setFeedback("");
                displayNameEdited.current = false;
                setDisplayNameScope({ id: null, label: "Everywhere" });
                setDisplayName(globalDisplayName ?? "");
                setEditor("displayname");
              }}
            >
              Edit
            </button>
          </div>
          {editor === "displayname" && (
            <form
              class="account-form account-editor"
              id="account-displayname-editor"
              onSubmit={(event) => {
                event.preventDefault();
                if (!displayNameScopeAvailable) return;
                updateDisplayName(
                  displayName.trim() || null,
                  displayNameScope.id ?? undefined,
                );
              }}
            >
              <label for="account-displayname">Display name</label>
              <div class="account-displayname-field">
                <input
                  id="account-displayname"
                  ref={editorInput}
                  value={displayName}
                  disabled={savingDisplayName}
                  autoComplete="nickname"
                  maxLength={256}
                  placeholder={
                    displayNameScope.id !== null
                      ? (globalDisplayName ?? state.username)
                      : state.username
                  }
                  onInput={(event) => {
                    displayNameEdited.current = true;
                    setDisplayName(event.currentTarget.value);
                  }}
                />
                <ScopePicker
                  id="displayname-scope"
                  label="Display name scope"
                  selected={{
                    id: displayNameScope.id,
                    label: displayNameRoom
                      ? `#${displayNameRoom.name}`
                      : displayNameScope.label,
                  }}
                  scopes={[
                    { id: null, label: "Everywhere" },
                    ...displayNameRooms.map((room) => ({
                      id: room.id,
                      label: `#${room.name}`,
                    })),
                  ]}
                  saving={savingDisplayName}
                  onChoose={(id) => {
                    const room = displayNameRooms.find(
                      (candidate) => candidate.id === id,
                    );
                    setDisplayNameScope({
                      id,
                      label: room ? `#${room.name}` : "Everywhere",
                    });
                    setFeedback("");
                  }}
                />
              </div>
              {!displayNameScopeAvailable && (
                <p class="account-error" role="alert">
                  Choose an available scope before saving.
                </p>
              )}
              <div class="account-editor-actions">
                <button
                  type="submit"
                  disabled={
                    pending || savingDisplayName || !displayNameScopeAvailable
                  }
                >
                  {savingDisplayName ? "Saving…" : "Save"}
                </button>
                <button
                  type="button"
                  class="account-cancel"
                  disabled={savingDisplayName}
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
              role={failed ? "alert" : "status"}
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
              class="account-setting-action"
              aria-expanded={editor === "password"}
              aria-controls="account-password-editor"
              disabled={pending || !canChangePassword || editor === "password"}
              onClick={(event) => {
                editorButton.current = event.currentTarget;
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
                  {pending ? "Saving…" : "Save password"}
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
              role={failed ? "alert" : "status"}
            >
              {feedback}
            </p>
          )}
        </>
      )}
    </div>
  );
}
