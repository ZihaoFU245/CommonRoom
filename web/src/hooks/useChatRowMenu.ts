import type { JSX } from "preact";
import { useLayoutEffect, useRef, useState } from "preact/hooks";
import { useEvent } from "./useEvent.ts";

type RowEvent = JSX.TargetedPointerEvent<HTMLButtonElement>;

export function useChatRowMenu(mobile: boolean, active: boolean) {
  const trigger = useRef<HTMLButtonElement>(null);
  const [menu, setMenu] = useState<{
    view: string;
    position: { x: number; y: number; keyboard: boolean; touchHold?: true };
  }>();
  const press = useRef<{
    timer: number;
    id: number;
    x: number;
    y: number;
  }>();
  const heldRow = useRef<HTMLButtonElement>();

  const cancelPress = useEvent(() => {
    if (!press.current) return;
    clearTimeout(press.current.timer);
    press.current = undefined;
  });
  const close = useEvent(() => setMenu(undefined));
  useLayoutEffect(() => {
    close();
    if (mobile && active) window.addEventListener("scroll", cancelPress, true);
    return () => {
      cancelPress();
      heldRow.current = undefined;
      window.removeEventListener("scroll", cancelPress, true);
    };
  }, [mobile, active, cancelPress, close]);

  function open(
    view: string,
    button: HTMLButtonElement,
    position: NonNullable<typeof menu>["position"],
  ) {
    if (!active) return;
    trigger.current = button;
    setMenu({ view, position });
  }

  function openFromButton(
    view: string,
    button: HTMLButtonElement,
    keyboard: boolean,
  ) {
    if (menu?.view === view && trigger.current === button && !keyboard) {
      close();
      return;
    }
    const bounds = button.getBoundingClientRect();
    open(view, button, { x: bounds.left, y: bounds.bottom, keyboard });
  }

  return {
    menu,
    trigger,
    close,
    openFromButton,
    rowEvents(view: string) {
      return {
        onPointerDown(event: RowEvent) {
          cancelPress();
          heldRow.current = undefined;
          if (
            !mobile ||
            !active ||
            !event.isPrimary ||
            event.pointerType !== "touch"
          )
            return;
          const {
            currentTarget: row,
            pointerId: id,
            clientX: x,
            clientY: y,
          } = event;
          const timer = window.setTimeout(() => {
            heldRow.current = row;
            open(view, row, { x, y, keyboard: false, touchHold: true });
          }, 500);
          press.current = { timer, id, x, y };
        },
        onPointerMove(event: RowEvent) {
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
        onClickCapture(event: JSX.TargetedMouseEvent<HTMLButtonElement>) {
          if (!event.detail || heldRow.current !== event.currentTarget) return;
          heldRow.current = undefined;
          event.preventDefault();
          event.stopPropagation();
        },
        onContextMenu(event: JSX.TargetedMouseEvent<HTMLButtonElement>) {
          event.preventDefault();
          if (!mobile)
            open(view, event.currentTarget, {
              x: event.clientX,
              y: event.clientY,
              keyboard: false,
            });
        },
        onKeyDown(event: JSX.TargetedKeyboardEvent<HTMLButtonElement>) {
          if (
            event.key !== "ContextMenu" &&
            !(event.shiftKey && event.key === "F10")
          )
            return;
          event.preventDefault();
          openFromButton(view, event.currentTarget, true);
        },
      };
    },
  };
}
