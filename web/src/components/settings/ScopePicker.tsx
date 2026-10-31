import { useEvent } from "../../hooks/useEvent.ts";
import { useLayoutEffect, useRef, useState } from "preact/hooks";
import type { JSX } from "preact";
import { useSettingsMotion } from "../../hooks/useSettingsMotion.ts";

export function focusSettingsControl(
  event: JSX.TargetedMouseEvent<HTMLButtonElement>,
) {
  if (event.button !== 0) return;
  // macOS Safari can focus the surrounding dialog instead of a clicked button,
  // causing the outside-focus listener to dismiss the menu before activation.
  event.preventDefault();
  event.currentTarget.focus({ preventScroll: true });
}

export function Chevron() {
  return (
    <svg class="name-color-chevron" viewBox="0 0 16 16" aria-hidden="true">
      <path d="m4 6 4 4 4-4" />
    </svg>
  );
}

export function useSettingsMenu() {
  const [open, setOpen] = useState(false);
  const container = useSettingsMotion<HTMLDivElement>();
  const toggle = useRef<HTMLButtonElement>(null);
  const close = useEvent((restoreFocus = false) => {
    // AutoAnimate retains exiting elements briefly; remove them from keyboard navigation.
    container.current
      ?.querySelector("[data-name-popup]:not([inert])")
      ?.setAttribute("inert", "");
    setOpen(false);
    if (restoreFocus) toggle.current?.focus();
  });
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
  }, [open, close, container]);
  return {
    open,
    container,
    toggle,
    close,
    toggleOpen: () => (open ? close() : setOpen(true)),
    show: () => setOpen(true),
  };
}

interface Scope {
  id: string | null;
  label: string;
}
export function ScopePicker({
  id,
  label,
  labelId,
  scopes,
  selected,
  saving,
  onChoose,
}: {
  id: string;
  label: string;
  labelId?: string;
  scopes: Scope[];
  selected: Scope;
  saving: boolean;
  onChoose: (id: string | null) => void;
}) {
  const menu = useSettingsMenu();
  const search = useRef({ text: "", time: 0 });
  return (
    <div class="name-color-control" ref={menu.container}>
      <button
        id={id}
        type="button"
        class="name-color-toggle name-scope-toggle"
        ref={menu.toggle}
        aria-haspopup="listbox"
        aria-expanded={menu.open}
        aria-controls={`${id}s`}
        aria-label={labelId ? undefined : label}
        aria-labelledby={labelId ? `${labelId} ${id}-value` : undefined}
        disabled={saving}
        onMouseDown={focusSettingsControl}
        onClick={menu.toggleOpen}
        onKeyDown={(event) => {
          if (["ArrowDown", "ArrowUp"].includes(event.key)) {
            event.preventDefault();
            menu.show();
          }
        }}
      >
        <span id={`${id}-value`} title={selected.label}>
          {selected.label}
        </span>
        <Chevron />
      </button>
      {menu.open && (
        <div
          id={`${id}s`}
          data-name-popup
          class="name-scope-options"
          role="listbox"
          aria-label={labelId ? undefined : label}
          aria-labelledby={labelId}
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
              onMouseDown={focusSettingsControl}
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
  );
}
