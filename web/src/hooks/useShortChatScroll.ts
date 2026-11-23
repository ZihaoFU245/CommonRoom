import type { JSX, RefObject } from "preact";
import { useLayoutEffect, useRef } from "preact/hooks";
import { useEvent } from "./useEvent.ts";

type ListPointer = JSX.TargetedPointerEvent<HTMLElement>;
const slop = 10;
const maxPull = 56;

/** Give a short mobile list elastic travel without replacing native scrolling. */
export function useShortChatScroll(
  enabled: boolean,
  list: RefObject<HTMLOListElement>,
) {
  const navigation = useRef<HTMLElement>(null);
  const limit = useRef(0);
  const pointer = useRef<{ id: number; x: number; y: number }>();
  const suppressClick = useRef(false);

  function pull(distance: number) {
    const element = list.current;
    const cap = limit.current;
    if (!element || !cap) return;
    const offset = (cap * distance) / (distance + cap);
    element.setAttribute("data-chat-pulling", "");
    element.style.setProperty("--chat-list-pull", `${offset}px`);
  }

  const release = useEvent(() => {
    pointer.current = undefined;
    list.current?.removeAttribute("data-chat-pulling");
    list.current?.style.removeProperty("--chat-list-pull");
  });

  useLayoutEffect(() => {
    const viewport = navigation.current;
    const content = list.current;
    if (
      !enabled ||
      !viewport ||
      !content ||
      typeof ResizeObserver !== "function"
    )
      return;
    const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
    const measure = () => {
      const spare = viewport.clientHeight - content.offsetHeight - 1;
      limit.current = reducedMotion.matches
        ? 0
        : Math.max(0, Math.min(maxPull, spare));
      viewport.toggleAttribute("data-short-chat-list", limit.current > 0);
      if (!limit.current) release();
    };
    const observer = new ResizeObserver(measure);
    observer.observe(viewport);
    observer.observe(content);
    reducedMotion.addEventListener("change", measure);
    measure();
    return () => {
      observer.disconnect();
      reducedMotion.removeEventListener("change", measure);
      viewport.removeAttribute("data-short-chat-list");
      limit.current = 0;
      release();
    };
  }, [enabled, list, release]);

  function finishPointer(event: ListPointer) {
    if (pointer.current?.id === event.pointerId) release();
  }

  return {
    ref: navigation,
    onPointerDown(event: ListPointer) {
      if (!event.isPrimary) {
        if (pointer.current) release();
        return;
      }
      suppressClick.current = false;
      if (!limit.current || event.pointerType !== "touch") return;
      release();
      pointer.current = {
        id: event.pointerId,
        x: event.clientX,
        y: event.clientY,
      };
    },
    onPointerMove(event: ListPointer) {
      const start = pointer.current;
      if (!start || start.id !== event.pointerId) return;
      const dx = Math.abs(event.clientX - start.x);
      const dy = event.clientY - start.y;
      if (!suppressClick.current) {
        if (Math.abs(dy) <= slop && dx <= slop) return;
        if (dx >= Math.abs(dy)) {
          release();
          return;
        }
        suppressClick.current = true;
      }
      pull(Math.max(0, dy - slop));
    },
    onPointerUp: finishPointer,
    onPointerCancel: finishPointer,
    onLostPointerCapture: finishPointer,
    onClickCapture(event: JSX.TargetedMouseEvent<HTMLElement>) {
      if (!event.detail || !suppressClick.current) return;
      suppressClick.current = false;
      event.preventDefault();
      event.stopPropagation();
    },
  };
}
