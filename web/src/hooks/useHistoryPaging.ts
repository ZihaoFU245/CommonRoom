import type { RefObject } from "preact";
import { useEffect } from "preact/hooks";
import { useEvent } from "./useEvent.ts";

/** Boundary gestures must work even when the loaded page is too short to scroll. */
export function useHistoryPaging(
  container: RefObject<HTMLElement>,
  ready: boolean,
  loading: boolean,
  before: boolean,
  after: boolean,
  loadOlder: () => void,
  loadNewer: () => void,
) {
  const page = useEvent((direction: number) => {
    const element = container.current;
    if (!element || !ready || loading) return;
    if (direction < 0 && before && element.scrollTop < element.clientHeight)
      loadOlder();
    else if (
      direction > 0 &&
      after &&
      element.scrollHeight - element.scrollTop - element.clientHeight <
        element.clientHeight
    )
      loadNewer();
  });
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    let touchY: number | undefined;
    const wheel = (event: WheelEvent) => {
      if (!event.ctrlKey) page(event.deltaY);
    };
    const touchstart = (event: TouchEvent) => {
      touchY = event.touches[0]?.clientY;
    };
    const touchmove = (event: TouchEvent) => {
      const next = event.touches[0]?.clientY;
      if (next !== undefined && touchY !== undefined) page(touchY - next);
      touchY = next;
    };
    const keydown = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.ctrlKey ||
        event.metaKey ||
        event.altKey ||
        event.shiftKey
      )
        return;
      if (
        event.target instanceof Element &&
        event.target.closest("input, textarea, select, [contenteditable]")
      )
        return;
      if (["PageUp", "Home", "ArrowUp"].includes(event.key)) page(-1);
      else if (["PageDown", "End", "ArrowDown"].includes(event.key)) page(1);
    };
    element.addEventListener("wheel", wheel, { passive: true });
    element.addEventListener("touchstart", touchstart, { passive: true });
    element.addEventListener("touchmove", touchmove, { passive: true });
    element.addEventListener("keydown", keydown);
    return () => {
      element.removeEventListener("wheel", wheel);
      element.removeEventListener("touchstart", touchstart);
      element.removeEventListener("touchmove", touchmove);
      element.removeEventListener("keydown", keydown);
    };
  }, [container, page]);
}
