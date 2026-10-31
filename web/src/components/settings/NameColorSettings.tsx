import {
  ScopePicker,
  Chevron,
  useSettingsMenu,
  focusSettingsControl,
} from "./ScopePicker.tsx";
import { useLayoutEffect, useRef, useState } from "preact/hooks";
import { saveErrorMessage } from "../../features/preferences/saveFeedback.ts";
import {
  nameColorLabel,
  nameColorOf,
  nameColors,
  type NameColor,
} from "../../features/preferences/nameColors.ts";
import type { SettingsProps } from "./types.ts";

const colorPermission = "w:account.color.own";

interface ColorScope {
  id: string | null;
  label: string;
  colors: Record<string, string> | undefined;
}
function ScopeField({
  scopes,
  selected,
  saving,
  onChoose,
}: {
  scopes: ColorScope[];
  selected: ColorScope;
  saving: boolean;
  onChoose: (id: string | null) => void;
}) {
  return (
    <div class="name-color-setting name-color-scope">
      <span id="name-color-scope-label">Name color scope</span>
      <ScopePicker
        id="name-color-scope"
        label="Name color scope"
        labelId="name-color-scope-label"
        scopes={scopes}
        selected={selected}
        saving={saving}
        onChoose={onChoose}
      />
    </div>
  );
}

function NameColorField({
  value,
  fallback,
  fallbackColor,
  username,
  saving,
  onChoose,
}: {
  value: NameColor | undefined;
  fallback: string;
  fallbackColor: NameColor | undefined;
  username: string;
  saving: boolean;
  onChoose: (color: NameColor | null) => Promise<void>;
}) {
  const menu = useSettingsMenu();
  const [draft, setDraft] = useState<NameColor | null>(value ?? null);
  const [failure, setFailure] = useState("");
  const applying = useRef(false);
  const failureElement = useRef<HTMLParagraphElement>(null);
  useLayoutEffect(() => {
    if (failure && menu.open)
      failureElement.current?.scrollIntoView({ block: "nearest" });
  }, [failure, menu.open]);
  function toggle() {
    if (saving) return;
    if (!menu.open && !failure) setDraft(value ?? null);
    menu.toggleOpen();
  }
  function apply() {
    if (saving || applying.current) return;
    applying.current = true;
    setFailure("");
    onChoose(draft)
      .then(() => {
        menu.close(menu.container.current?.contains(document.activeElement));
      })
      .catch((error: unknown) => setFailure(saveErrorMessage(error)))
      .finally(() => {
        applying.current = false;
      });
  }
  const options: { color: NameColor | null; label: string }[] = [
    { color: null, label: fallback },
    ...nameColors.map((color) => ({ color, label: nameColorLabel(color) })),
  ];
  return (
    <div class="name-color-setting">
      <span id="name-color-label">Name color</span>
      <div class="name-color-control" ref={menu.container}>
        <button
          ref={menu.toggle}
          type="button"
          class="name-color-toggle"
          aria-haspopup="dialog"
          aria-controls="name-color-options"
          aria-expanded={menu.open}
          aria-labelledby="name-color-label name-color-value"
          aria-disabled={saving}
          onMouseDown={focusSettingsControl}
          onClick={toggle}
        >
          <span
            class="name-color-swatch"
            data-name-color={value ?? fallbackColor}
          />
          <span id="name-color-value">
            {value ? nameColorLabel(value) : fallback}
          </span>
          <Chevron />
        </button>
        {menu.open && (
          <div
            id="name-color-options"
            class="name-color-picker"
            data-name-popup
            role="dialog"
            aria-label="Name colors"
            aria-busy={saving}
            onKeyDown={(event) => {
              if (event.key !== "Escape") return;
              event.preventDefault();
              event.stopPropagation();
              menu.close(true);
            }}
          >
            <div class="name-color-preview" aria-live="polite">
              <strong
                class="message-author"
                data-name-color={draft ?? fallbackColor}
              >
                {username}
              </strong>
              <span>{draft ? nameColorLabel(draft) : fallback}</span>
            </div>
            {options.map(({ color, label }) => (
              <button
                key={label}
                type="button"
                class="name-color-option"
                data-name-color={color ?? fallbackColor}
                aria-pressed={color === draft}
                aria-label={label}
                title={label}
                disabled={saving}
                onMouseDown={focusSettingsControl}
                onClick={() => {
                  setFailure("");
                  setDraft(color);
                }}
              />
            ))}
            <div class="name-color-actions">
              <button
                type="button"
                disabled={saving}
                onMouseDown={focusSettingsControl}
                onClick={() => {
                  setFailure("");
                  menu.close(true);
                }}
              >
                Cancel
              </button>
              <button
                type="button"
                class="primary"
                aria-disabled={saving}
                onMouseDown={focusSettingsControl}
                onClick={(event) => {
                  event.currentTarget.focus({ preventScroll: true });
                  apply();
                }}
              >
                {saving ? "Saving…" : "Apply"}
              </button>
            </div>
            {failure && (
              <p ref={failureElement} class="account-error" role="alert">
                {failure}
              </p>
            )}
          </div>
        )}
        {!menu.open && failure && (
          <p class="account-error" role="alert">
            {failure}
          </p>
        )}
      </div>
    </div>
  );
}

/** Scopes come from current grants; the server rechecks each change. */
export function NameColorSettings({
  state,
  onNameColorChange,
}: Pick<SettingsProps, "state" | "onNameColorChange">) {
  const scopes = [
    { id: null, label: "Everywhere", colors: state.colors },
    ...state.rooms
      .filter(
        (room) =>
          !room.name.startsWith("@") &&
          (room.permissions.includes(colorPermission) ||
            room.permissions.includes("w:account.color.any")),
      )
      .map((room) => ({
        id: room.id,
        label: `#${room.name}`,
        colors: room.colors,
      })),
  ];
  const [selected, setSelected] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [feedback, setFeedback] = useState("");
  const scope = scopes.find((item) => item.id === selected) ?? scopes[0];
  if (!scope) return null;
  const value = nameColorOf(scope.colors ?? {}, state.username);
  async function change(color: NameColor | null) {
    if (!scope || (color ?? undefined) === value) return;
    setSaving(true);
    setFeedback("");
    try {
      await onNameColorChange(color, scope.id);
      setFeedback("Name color saved.");
    } finally {
      setSaving(false);
    }
  }
  return (
    <>
      <ScopeField
        scopes={scopes}
        selected={scope}
        saving={saving}
        onChoose={setSelected}
      />
      <NameColorField
        key={scope.id ?? "global"}
        value={value}
        username={state.username}
        fallbackColor={
          scope.id === null
            ? undefined
            : nameColorOf(state.colors ?? {}, state.username)
        }
        fallback={scope.id === null ? "Default" : "Same as everywhere"}
        saving={saving}
        onChoose={change}
      />
      {feedback && (
        <p class="account-feedback" role="status">
          {feedback}
        </p>
      )}
    </>
  );
}
