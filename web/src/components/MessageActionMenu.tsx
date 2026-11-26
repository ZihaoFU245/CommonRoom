import { createPortal } from "preact/compat";
import { createContext } from "preact";
import type { JSX, RefObject, VNode } from "preact";
import { useLayoutEffect, useRef, useState } from "preact/hooks";
import {
  canSwipeInPage,
  focusControl,
  touchControls,
} from "../features/preferences/inputFocus.ts";
import { useEvent } from "../hooks/useEvent.ts";

const interactive =
  "a, button, summary, input, textarea, select, video, audio, iframe, pre, code, [contenteditable], [role='button']";
const messageImage = ".message-image, a:has(.message-image)";
const replyDistance = 64;

type MenuPosition = {
  x: number;
  y: number;
  keyboard: boolean;
  touchHold?: true;
  sticker?: HTMLElement | undefined;
  media?: Element | null;
};

export const StickerActionsContext = createContext<
  | ((element: HTMLElement, action: MessageAction | undefined) => () => void)
  | undefined
>(undefined);

export function useMessageActionMenu(
  enabled: boolean,
  onSwipeReply?: () => void,
) {
  const trigger = useRef<HTMLButtonElement>(null);
  const [stickers] = useState(
    () => new WeakMap<HTMLElement, MessageAction | undefined>(),
  );
  const heldMedia = useRef<Element>();
  const press = useRef<{
    id: number;
    x: number;
    y: number;
    timer: number;
    target: Element;
    replyRow: HTMLElement | undefined;
    held: boolean;
    swiping: boolean;
  }>();
  const [position, setPosition] = useState<MenuPosition>();
  const registerSticker = useEvent(
    (element: HTMLElement, action: MessageAction | undefined) => {
      stickers.set(element, action);
      return () => {
        stickers.delete(element);
        setPosition((current) =>
          current?.sticker === element ? undefined : current,
        );
      };
    },
  );

  const cancelPress = useEvent(() => {
    const active = press.current;
    if (!active) return;
    clearTimeout(active.timer);
    active.target.removeAttribute("data-touch-held");
    active.replyRow?.removeAttribute("data-reply-swiping");
    active.replyRow?.removeAttribute("data-reply-ready");
    active.replyRow?.style.removeProperty("--reply-swipe-x");
    active.replyRow?.style.removeProperty("--reply-swipe-progress");
    window.removeEventListener("scroll", onPressScroll, true);
    press.current = undefined;
  });

  const onPressScroll = useEvent(() => {
    const active = press.current;
    if (!active?.replyRow || active.held) {
      cancelPress();
      return;
    }
    // Layout changes can scroll the timeline while a touch is still active.
    // Dismiss the hold; pointer cancellation handles native touch scrolling.
    clearTimeout(active.timer);
    active.target.removeAttribute("data-touch-held");
  });

  useLayoutEffect(() => {
    if (!enabled) setPosition(undefined);
    return cancelPress;
  }, [enabled, cancelPress]);

  function openFromTrigger(keyboard: boolean) {
    const bounds = trigger.current?.getBoundingClientRect();
    if (bounds) setPosition({ x: bounds.left, y: bounds.bottom, keyboard });
  }

  function stickerAt(target: Element) {
    const sticker = target.closest<HTMLElement>(".sticker-hold-surface");
    return sticker && stickers.has(sticker) ? sticker : undefined;
  }

  return {
    trigger,
    registerSticker,
    position: enabled ? position : undefined,
    stickerAction: position?.sticker
      ? stickers.get(position.sticker)
      : undefined,
    close: () => setPosition(undefined),
    openFromTrigger,
    gesture: {
      onPointerDown(event: JSX.TargetedPointerEvent<HTMLElement>) {
        cancelPress();
        heldMedia.current = undefined;
        if (
          !enabled ||
          position ||
          !event.isPrimary ||
          event.pointerType !== "touch" ||
          !(event.target instanceof Element)
        )
          return;
        const sticker = stickerAt(event.target);
        const image = event.target.closest(messageImage);
        const interactiveTarget = event.target.closest(interactive);
        const media = sticker ?? image;
        const control = media ? null : interactiveTarget;
        const { clientX: x, clientY: y, pointerId: id } = event;
        const target = media ?? control ?? event.currentTarget;
        const replyRow =
          onSwipeReply && !interactiveTarget && canSwipeInPage(x)
            ? event.currentTarget
            : undefined;
        if (sticker) target.setAttribute("data-touch-held", "");
        const timer = window.setTimeout(() => {
          if (press.current) press.current.held = true;
          if (media) heldMedia.current = media;
          else if (control) target.setAttribute("data-touch-held", "");
          if (!control)
            setPosition({
              x,
              y,
              keyboard: false,
              touchHold: true,
              sticker,
              media,
            });
        }, 500);
        press.current = {
          id,
          x,
          y,
          timer,
          target,
          replyRow,
          held: false,
          swiping: false,
        };
        window.addEventListener("scroll", onPressScroll, {
          capture: true,
          passive: true,
        });
      },
      onPointerMove(event: JSX.TargetedPointerEvent<HTMLElement>) {
        const start = press.current;
        if (start?.id !== event.pointerId) return;
        const dx = event.clientX - start.x;
        const dy = event.clientY - start.y;
        const row = start.replyRow;
        if (!start.swiping) {
          if (Math.hypot(dx, dy) <= 10) return;
          if (!row || start.held || Math.abs(dx) <= Math.abs(dy) * 1.4) {
            cancelPress();
            return;
          }
          start.swiping = true;
          clearTimeout(start.timer);
          start.target.removeAttribute("data-touch-held");
        }
        if (!row) return;
        const offset = Math.max(-88, Math.min(88, dx));
        row.dataset.replySwiping = offset < 0 ? "left" : "right";
        row.toggleAttribute("data-reply-ready", Math.abs(dx) >= replyDistance);
        row.style.setProperty("--reply-swipe-x", `${offset}px`);
        row.style.setProperty(
          "--reply-swipe-progress",
          String(
            Math.max(
              0,
              Math.min(1, (Math.abs(offset) - 26) / (replyDistance - 26)),
            ),
          ),
        );
      },
      onPointerUp(event: JSX.TargetedPointerEvent<HTMLElement>) {
        const start = press.current;
        const reply =
          start?.id === event.pointerId &&
          start.swiping &&
          Math.abs(event.clientX - start.x) >= replyDistance;
        cancelPress();
        if (reply && onSwipeReply) {
          onSwipeReply();
          navigator.vibrate?.(12);
        }
      },
      onPointerCancel: cancelPress,
      onLostPointerCapture: cancelPress,
      onClickCapture(event: JSX.TargetedMouseEvent<HTMLElement>) {
        const media = heldMedia.current;
        if (!event.detail || !media || !(event.target instanceof Element))
          return;
        if (
          !media.contains(event.target) &&
          event.target !== media.closest(interactive)
        )
          return;
        heldMedia.current = undefined;
        // Releasing a hold must not activate the underlying image link.
        event.preventDefault();
        event.stopPropagation();
      },
      onContextMenu(event: JSX.TargetedMouseEvent<HTMLElement>) {
        const pointer = "pointerType" in event ? event.pointerType : undefined;
        const touch = pointer
          ? pointer === "touch"
          : Boolean(press.current) || matchMedia(touchControls).matches;
        if (
          enabled &&
          touch &&
          event.target instanceof Element &&
          (stickerAt(event.target) ||
            event.target.closest(messageImage) ||
            !event.target.closest(interactive))
        )
          event.preventDefault();
      },
    },
  };
}

export type MessageAction = {
  label: string;
  accessibleLabel: string;
  description?: string | undefined;
  icon?: VNode;
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
  id,
  label = "Message actions",
  className = "",
}: {
  position: MenuPosition;
  trigger: RefObject<HTMLButtonElement>;
  actions: MessageAction[];
  reaction?:
    { label: string; render: (onDone: () => void) => VNode } | undefined;
  copyText?: string;
  onClose: () => void;
  id?: string;
  label?: string;
  className?: string;
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
      // The release that completes a hold lands outside the newly opened menu.
      // Let the existing outside pointer-down handler dismiss it on the next tap.
      element.setAttribute("popover", position.touchHold ? "manual" : "auto");
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
      const media = position.media?.getBoundingClientRect();
      const image = position.sticker
        ?.querySelector("img")
        ?.getBoundingClientRect();
      const mediaBottom = media?.bottom ?? position.y;
      const mediaTop = media?.top ?? position.y;
      const below = Math.max(mediaBottom, image?.bottom ?? mediaBottom) + 8;
      const above =
        Math.min(mediaTop, image?.top ?? mediaTop) - 8 - bounds.height;
      const nextTop =
        below + bounds.height <= top + height - 8
          ? below
          : above >= top + 8
            ? above
            : position.y - 8 - bounds.height;
      element.style.left = `${Math.max(left + 8, Math.min(position.x, left + width - bounds.width - 8))}px`;
      element.style.top = `${Math.max(top + 8, Math.min(nextTop, top + height - bounds.height - 8))}px`;
      element.style.visibility = "visible";
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
      const media = position.media;
      if (
        event.target instanceof Node &&
        !element.contains(event.target) &&
        !trigger.current?.contains(event.target) &&
        !(
          event.type === "focusin" &&
          media &&
          (media.contains(event.target) ||
            event.target === media.closest(interactive))
        )
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
    const toggled = () => {
      if (!element.matches(":popover-open")) dismiss();
    };
    element.addEventListener("toggle", toggled);
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("focusin", outside);
    document.addEventListener("keydown", escape, true);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("resize", place);
    window.addEventListener("popstate", dismiss);
    viewport?.addEventListener("resize", place);
    viewport?.addEventListener("scroll", place);
    return () => {
      element.removeEventListener("toggle", toggled);
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("focusin", outside);
      document.removeEventListener("keydown", escape, true);
      window.removeEventListener("scroll", scroll, true);
      window.removeEventListener("resize", place);
      window.removeEventListener("popstate", dismiss);
      viewport?.removeEventListener("resize", place);
      viewport?.removeEventListener("scroll", place);
    };
  }, [position, trigger, mode, showingPanel]);

  function complete() {
    if (position.keyboard)
      focusControl(trigger.current, { preventScroll: true });
    onClose();
  }

  async function copyMessage() {
    if (!copyText) return;
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
      class={`message-action-menu ${showingPanel ? "message-action-panel" : ""} ${className}`}
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
          id={id}
          {...(nativeFocus
            ? { focusgroup: "menu nomemory" }
            : { role: "menu" })}
          class="message-action-items"
          aria-label={label}
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
              aria-label={
                action.description
                  ? `${action.accessibleLabel}. ${action.description}`
                  : action.accessibleLabel
              }
              onClick={() => {
                complete();
                action.run();
              }}
            >
              {action.icon}
              {action.label}
              {action.description && <small>{action.description}</small>}
            </button>
          ))}
        </div>
      )}
    </div>,
    document.body,
  );
}
