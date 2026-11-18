import { createPortal } from "preact/compat";
import type { JSX, RefObject, VNode } from "preact";
import { useLayoutEffect, useRef, useState } from "preact/hooks";
import {
  focusControl,
  touchControls,
} from "../features/preferences/inputFocus.ts";
import { useEvent } from "../hooks/useEvent.ts";

const interactive =
  "a, button, summary, input, textarea, select, video, audio, iframe, pre, code, [contenteditable], [role='button']";

type MenuPosition = { x: number; y: number; keyboard: boolean };

export function useMessageActionMenu(enabled: boolean) {
  const trigger = useRef<HTMLButtonElement>(null);
  const press = useRef<{
    id: number;
    x: number;
    y: number;
    timer: number;
    row: HTMLElement;
  }>();
  const [position, setPosition] = useState<MenuPosition>();

  const cancelPress = useEvent(() => {
    const active = press.current;
    if (!active) return;
    clearTimeout(active.timer);
    delete active.row.dataset.touchPressed;
    press.current = undefined;
  });

  useLayoutEffect(() => {
    if (!enabled) setPosition(undefined);
    return cancelPress;
  }, [enabled, cancelPress]);

  function openFromTrigger(keyboard: boolean) {
    const bounds = trigger.current?.getBoundingClientRect();
    if (bounds) setPosition({ x: bounds.left, y: bounds.bottom, keyboard });
  }

  return {
    trigger,
    position: enabled ? position : undefined,
    close: () => setPosition(undefined),
    openFromTrigger,
    gesture: {
      onPointerDown(event: JSX.TargetedPointerEvent<HTMLElement>) {
        cancelPress();
        if (
          !enabled ||
          position ||
          !event.isPrimary ||
          event.pointerType !== "touch" ||
          !(event.target instanceof Element) ||
          event.target.closest(interactive)
        )
          return;
        const { clientX: x, clientY: y, pointerId: id } = event;
        const row = event.currentTarget;
        row.dataset.touchPressed = "";
        const timer = window.setTimeout(() => {
          setPosition({ x, y, keyboard: false });
        }, 500);
        press.current = { id, x, y, timer, row };
      },
      onPointerMove(event: JSX.TargetedPointerEvent<HTMLElement>) {
        const start = press.current;
        if (
          start?.id === event.pointerId &&
          Math.hypot(event.clientX - start.x, event.clientY - start.y) > 10
        )
          cancelPress();
      },
      onPointerUp: cancelPress,
      onPointerCancel: cancelPress,
      onLostPointerCapture: cancelPress,
      onContextMenu(event: JSX.TargetedMouseEvent<HTMLElement>) {
        const pointer = "pointerType" in event ? event.pointerType : undefined;
        const touch = pointer
          ? pointer === "touch"
          : Boolean(press.current) || matchMedia(touchControls).matches;
        if (
          enabled &&
          touch &&
          event.target instanceof Element &&
          !event.target.closest(interactive)
        )
          event.preventDefault();
      },
    },
  };
}

export type MessageAction = {
  label: string;
  accessibleLabel: string;
  disabled: boolean;
  run: () => void;
};

export function MessageActionMenu({
  position,
  trigger,
  actions,
  reaction,
  copyText,
  onClose,
}: {
  position: MenuPosition;
  trigger: RefObject<HTMLButtonElement>;
  actions: MessageAction[];
  reaction:
    { label: string; render: (onDone: () => void) => VNode } | undefined;
  copyText: string;
  onClose: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  const [mode, setMode] = useState<"actions" | "reaction" | "copy">("actions");
  const showingPanel = mode !== "actions";
  close.current = onClose;
  const nativeFocus = "focusGroup" in HTMLElement.prototype;

  useLayoutEffect(() => {
    const element = panel.current;
    if (!element) return;
    if (
      typeof element.showPopover === "function" &&
      !element.matches(":popover-open")
    ) {
      element.setAttribute("popover", "auto");
      element.showPopover();
    }
    const viewport = window.visualViewport;
    const place = () => {
      const left = viewport?.offsetLeft ?? 0;
      const top = viewport?.offsetTop ?? 0;
      const width = viewport?.width ?? innerWidth;
      const height = viewport?.height ?? innerHeight;
      element.style.maxHeight = `${Math.max(0, height - 16)}px`;
      element.style.maxWidth = `${Math.max(0, width - 16)}px`;
      const bounds = element.getBoundingClientRect();
      const below = position.y + 8;
      const nextTop =
        below + bounds.height <= top + height - 8
          ? below
          : position.y - 8 - bounds.height;
      element.style.left = `${Math.max(left + 8, Math.min(position.x, left + width - bounds.width - 8))}px`;
      element.style.top = `${Math.max(top + 8, Math.min(nextTop, top + height - bounds.height - 8))}px`;
    };
    place();
    if (mode === "copy") {
      const field = element.querySelector("textarea");
      focusControl(field, { preventScroll: true });
      field?.select();
    } else if (position.keyboard || showingPanel) {
      focusControl(element.querySelector<HTMLButtonElement>("button:enabled"), {
        preventScroll: true,
      });
    }

    const dismiss = () => close.current();
    const outside = (event: PointerEvent | FocusEvent) => {
      if (
        event.target instanceof Node &&
        !element.contains(event.target) &&
        !trigger.current?.contains(event.target)
      )
        dismiss();
    };
    const escape = (event: KeyboardEvent) => {
      if (
        !showingPanel &&
        event.key === "Tab" &&
        element.contains(document.activeElement)
      ) {
        focusControl(trigger.current, { preventScroll: true });
        dismiss();
        return;
      }
      if (event.key !== "Escape" || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      focusControl(trigger.current, { preventScroll: true });
      dismiss();
    };
    const scroll = (event: Event) => {
      if (event.target === element) return;
      if (showingPanel && element.contains(document.activeElement)) place();
      else dismiss();
    };
    const resize = showingPanel ? place : dismiss;
    const toggled = () => {
      if (!element.matches(":popover-open")) dismiss();
    };
    element.addEventListener("toggle", toggled);
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("focusin", outside);
    document.addEventListener("keydown", escape, true);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("resize", resize);
    viewport?.addEventListener("resize", resize);
    viewport?.addEventListener("scroll", resize);
    return () => {
      element.removeEventListener("toggle", toggled);
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("focusin", outside);
      document.removeEventListener("keydown", escape, true);
      window.removeEventListener("scroll", scroll, true);
      window.removeEventListener("resize", resize);
      viewport?.removeEventListener("resize", resize);
      viewport?.removeEventListener("scroll", resize);
    };
  }, [position, trigger, mode, showingPanel]);

  function complete() {
    if (position.keyboard)
      focusControl(trigger.current, { preventScroll: true });
    onClose();
  }

  async function copyMessage() {
    try {
      if (navigator.clipboard) {
        await navigator.clipboard.writeText(copyText);
        if (panel.current) complete();
        return;
      }
    } catch {
      // Permission denial and insecure HTTP both leave the manual copy path.
    }
    if (panel.current) setMode("copy");
  }

  function navigate(event: JSX.TargetedKeyboardEvent<HTMLDivElement>) {
    const { key } = event;
    if (
      key !== "Home" &&
      key !== "End" &&
      (nativeFocus || (key !== "ArrowUp" && key !== "ArrowDown"))
    )
      return;
    const items = [
      ...event.currentTarget.querySelectorAll<HTMLButtonElement>(
        "button:enabled",
      ),
    ];
    const index = items.findIndex((item) => item === document.activeElement);
    const next =
      key === "Home"
        ? 0
        : key === "End"
          ? items.length - 1
          : (index + (key === "ArrowUp" ? -1 : 1) + items.length) %
            items.length;
    event.preventDefault();
    focusControl(items[next], { preventScroll: true });
  }

  return createPortal(
    <div
      class={`message-action-menu ${showingPanel ? "message-action-panel" : ""}`}
      ref={panel}
    >
      {/* Reusing the menu node would remove focusgroup and warn in Chromium. */}
      {mode === "reaction" && reaction ? (
        <div
          key="reaction-picker"
          class="message-reaction-menu"
          role="dialog"
          aria-label={reaction.label}
        >
          {reaction.render(complete)}
        </div>
      ) : mode === "copy" ? (
        <div
          key="copy-message"
          class="message-copy-menu"
          role="dialog"
          aria-label="Copy message"
        >
          <p role="status">Select and copy the message text.</p>
          <textarea
            aria-label="Message text"
            readOnly
            rows={4}
            value={copyText}
            dir="auto"
          />
          <button type="button" onClick={complete}>
            Done
          </button>
        </div>
      ) : (
        <div
          key="message-actions"
          {...(nativeFocus
            ? { focusgroup: "menu nomemory" }
            : { role: "menu" })}
          class="message-action-items"
          aria-label="Message actions"
          onKeyDown={navigate}
        >
          {reaction && (
            <button
              type="button"
              role={nativeFocus ? undefined : "menuitem"}
              tabIndex={nativeFocus ? undefined : -1}
              aria-label={reaction.label}
              onClick={() => setMode("reaction")}
            >
              Add reaction
            </button>
          )}
          {copyText && (
            <button
              type="button"
              role={nativeFocus ? undefined : "menuitem"}
              tabIndex={nativeFocus ? undefined : -1}
              onClick={() => {
                copyMessage().catch((error: unknown) => {
                  console.error("Copy message failed", error);
                });
              }}
            >
              Copy message
            </button>
          )}
          {actions.map((action) => (
            <button
              key={action.label}
              type="button"
              role={nativeFocus ? undefined : "menuitem"}
              tabIndex={nativeFocus ? undefined : -1}
              disabled={action.disabled}
              data-destructive={action.label === "Delete" ? "" : undefined}
              aria-label={action.accessibleLabel}
              onClick={() => {
                complete();
                action.run();
              }}
            >
              {action.label}
            </button>
          ))}
        </div>
      )}
    </div>,
    document.body,
  );
}
