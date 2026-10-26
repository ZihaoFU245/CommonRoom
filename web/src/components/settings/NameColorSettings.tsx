import { useLayoutEffect, useRef, useState } from "preact/hooks";
import type { JSX } from "preact";
import { useSettingsMotion } from "../../hooks/useSettingsMotion.ts";
import { errorMessage } from "../../api/protocol.ts";
import {
  nameColorLabel,
  nameColorOf,
  nameColors,
  type NameColor,
} from "../../features/preferences/nameColors.ts";
import type { SettingsProps } from "./types.ts";

const colorPermission = "w:account.color.own";

function focusColorControl(event: JSX.TargetedMouseEvent<HTMLButtonElement>) {
  if (event.button !== 0) return;
  // macOS Safari can focus the surrounding dialog instead of a clicked button,
  // causing the outside-focus listener to dismiss the menu before activation.
  event.preventDefault();
  event.currentTarget.focus({ preventScroll: true });
}

function Chevron() {
  return (
    <svg class="name-color-chevron" viewBox="0 0 16 16" aria-hidden="true">
      <path d="m4 6 4 4 4-4" />
    </svg>
  );
}

function useColorMenu() {
  const [open, setOpen] = useState(false);
  const container = useSettingsMotion<HTMLDivElement>();
  const toggle = useRef<HTMLButtonElement>(null);
  function close(restoreFocus = false) {
    // AutoAnimate retains exiting elements briefly; remove them from keyboard navigation.
    container.current
      ?.querySelector("[data-name-popup]:not([inert])")
      ?.setAttribute("inert", "");
    setOpen(false);
    if (restoreFocus) toggle.current?.focus();
  }
  // Position and focus the popup before its entrance animation is painted.
  useLayoutEffect(() => {
    if (!open) return;
    const panel = container.current?.querySelector<HTMLElement>(
      "[data-name-popup]:not([inert])",
    );
    const anchor = container.current?.getBoundingClientRect();
    if (panel && anchor) {
      const bounds = container.current
        ?.closest(".settings-content")
        ?.getBoundingClientRect();
      const below =
        Math.min(bounds?.bottom ?? window.innerHeight, window.innerHeight) -
        anchor.bottom -
        6;
      const above = anchor.top - Math.max(bounds?.top ?? 0, 0) - 6;
      const upward =
        panel.getBoundingClientRect().height > below && above > below;
      panel.dataset["side"] = upward ? "top" : "bottom";
      panel.style.maxHeight = `${Math.max(80, Math.min(upward ? above : below, 256))}px`;
    }
    const selected =
      panel?.querySelector<HTMLButtonElement>(
        '[aria-selected="true"], [aria-pressed="true"]',
      ) ?? panel?.querySelector<HTMLButtonElement>("button");
    selected?.focus({ preventScroll: true });
    if (panel && selected)
      panel.scrollTop = Math.max(
        0,
        selected.offsetTop - panel.clientHeight / 2,
      );
    const outside = (event: Event) => {
      if (
        event.target instanceof Node &&
        !container.current?.contains(event.target)
      )
        close();
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("focusin", outside);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("focusin", outside);
    };
  }, [open]);
  return {
    open,
    container,
    toggle,
    close,
    toggleOpen: () => (open ? close() : setOpen(true)),
    show: () => setOpen(true),
  };
}

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
  const menu = useColorMenu();
  const search = useRef({ text: "", time: 0 });
  return (
    <div class="name-color-setting name-color-scope">
      <span id="name-color-scope-label">Name color scope</span>
      <div class="name-color-control" ref={menu.container}>
        <button
          id="name-color-scope"
          type="button"
          class="name-color-toggle name-scope-toggle"
          ref={menu.toggle}
          aria-haspopup="listbox"
          aria-expanded={menu.open}
          aria-controls="name-color-scopes"
          aria-labelledby="name-color-scope-label name-color-scope-value"
          disabled={saving}
          onMouseDown={focusColorControl}
          onClick={menu.toggleOpen}
          onKeyDown={(event) => {
            if (["ArrowDown", "ArrowUp"].includes(event.key)) {
              event.preventDefault();
              menu.show();
            }
          }}
        >
          <span id="name-color-scope-value" title={selected.label}>
            {selected.label}
          </span>
          <Chevron />
        </button>
        {menu.open && (
          <div
            id="name-color-scopes"
            data-name-popup
            class="name-scope-options"
            role="listbox"
            aria-labelledby="name-color-scope-label"
            onKeyDown={(event) => {
              const options = Array.from(
                event.currentTarget.querySelectorAll<HTMLButtonElement>(
                  '[role="option"]',
                ),
              );
              const index = options.findIndex(
                (option) => option === document.activeElement,
              );
              let next: HTMLButtonElement | undefined;
              switch (event.key) {
                case "Escape":
                  event.preventDefault();
                  event.stopPropagation();
                  menu.close(true);
                  return;
                case "ArrowDown":
                  next = options.at((index + 1) % options.length);
                  break;
                case "ArrowUp":
                  next = options.at(
                    (index - 1 + options.length) % options.length,
                  );
                  break;
                case "Home":
                  next = options[0];
                  break;
                case "End":
                  next = options.at(-1);
                  break;
                case "Tab":
                  menu.close(true);
                  return;
                default: {
                  if (
                    event.key.length !== 1 ||
                    event.ctrlKey ||
                    event.metaKey ||
                    event.altKey ||
                    event.key === " "
                  )
                    return;
                  const now = performance.now();
                  search.current = {
                    text:
                      (now - search.current.time < 700
                        ? search.current.text
                        : "") + event.key.toLocaleLowerCase(),
                    time: now,
                  };
                  next = options.find((option) =>
                    option.textContent
                      ?.trim()
                      .toLocaleLowerCase()
                      .startsWith(search.current.text),
                  );
                }
              }
              if (next) {
                event.preventDefault();
                next.focus();
              }
            }}
          >
            {scopes.map((scope) => (
              <button
                key={scope.id ?? "global"}
                type="button"
                class="name-scope-option"
                role="option"
                tabIndex={scope.id === selected.id ? 0 : -1}
                aria-selected={scope.id === selected.id}
                onMouseDown={focusColorControl}
                onClick={() => {
                  onChoose(scope.id);
                  menu.close(true);
                }}
              >
                <span title={scope.label}>{scope.label}</span>
                <svg viewBox="0 0 16 16" aria-hidden="true">
                  <path d="m3 8 3 3 7-7" />
                </svg>
              </button>
            ))}
          </div>
        )}
      </div>
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
  onChoose: (color: NameColor | null) => void;
}) {
  const menu = useColorMenu();
  const [draft, setDraft] = useState<NameColor | null>(value ?? null);
  function toggle() {
    if (saving) return;
    if (!menu.open) setDraft(value ?? null);
    menu.toggleOpen();
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
          onMouseDown={focusColorControl}
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
                onMouseDown={focusColorControl}
                onClick={() => setDraft(color)}
              />
            ))}
            <div class="name-color-actions">
              <button
                type="button"
                onMouseDown={focusColorControl}
                onClick={() => menu.close(true)}
              >
                Cancel
              </button>
              <button
                type="button"
                class="primary"
                onMouseDown={focusColorControl}
                onClick={() => {
                  menu.close(true);
                  onChoose(draft);
                }}
              >
                Apply
              </button>
            </div>
          </div>
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
  const [failure, setFailure] = useState("");
  const scope = scopes.find((item) => item.id === selected) ?? scopes[0];
  if (!scope) return null;
  const value = nameColorOf(scope.colors ?? {}, state.username);
  function change(color: NameColor | null) {
    if (!scope || (color ?? undefined) === value) return;
    setSaving(true);
    setFailure("");
    onNameColorChange(color, scope.id)
      .catch((error: unknown) => setFailure(errorMessage(error)))
      .finally(() => setSaving(false));
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
      {failure && (
        <p class="account-error" role="alert">
          {failure}
        </p>
      )}
    </>
  );
}
