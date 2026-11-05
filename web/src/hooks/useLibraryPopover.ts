import type { RefObject } from "preact";
import { useLayoutEffect, useRef } from "preact/hooks";
import { controlMotion } from "../features/preferences/motion.ts";
import { useEvent } from "./useEvent.ts";

/** Own the picker top-layer placement, motion and dismissal listeners. */
export function useLibraryPopover(
  anchor: RefObject<HTMLButtonElement> | undefined,
  closing: boolean,
  close: () => void,
) {
  const dialog = useRef<HTMLDialogElement>(null);
  const motion = useRef<Animation | undefined>(undefined);
  const exiting = useRef(false);
  const move = useEvent((opening: boolean) => {
    const element = dialog.current;
    if (!element) return;
    exiting.current = !opening;
    const previous = motion.current;
    const painted = previous ? getComputedStyle(element) : undefined;
    const opacity = painted ? Number(painted.opacity) : 0;
    const transform = painted?.transform ?? "scale(0.15)";
    previous?.cancel();
    motion.current = undefined;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      if (!opening) close();
      return;
    }
    const animation = element.animate(
      [
        { opacity, transform },
        {
          opacity: opening ? 1 : 0,
          transform: opening ? "scale(1)" : "scale(0.15)",
        },
      ],
      {
        ...controlMotion,
        duration:
          controlMotion.duration * Math.abs((opening ? 1 : 0) - opacity),
        fill: "both",
      },
    );
    motion.current = animation;
    animation.finished
      .then(
        () => {
          if (motion.current === animation && exiting.current) close();
        },
        () => undefined,
      )
      .catch((error: unknown) => {
        console.error("Picker dismissal failed", error);
      });
  });
  const dismiss = useEvent(() => {
    if (!exiting.current) move(false);
  });
  useLayoutEffect(() => {
    const element = dialog.current;
    if (!element) return;
    element.showPopover();
    // Reopening with a queued attachment disables previews. Keep the browser
    // from focusing the first remove control and revealing its actions.
    element.focus({ preventScroll: true });
    const position = () => {
      const visual = window.visualViewport;
      const left = visual?.offsetLeft ?? 0;
      const top = visual?.offsetTop ?? 0;
      const width = visual?.width ?? innerWidth;
      const height = visual?.height ?? innerHeight;
      const bounds = anchor?.current?.getBoundingClientRect();
      element.style.width = `${Math.min(420, width - 24)}px`;
      const bottom = bounds
        ? Math.min(bounds.top - 8, top + height - 12)
        : top + height - 24;
      element.style.height = `${Math.min(460, Math.max(120, bottom - top - 12))}px`;
      // Entry motion scales the painted box; positioning uses its layout size.
      element.style.left = `${Math.max(left + 12, Math.min(bounds?.left ?? left + 12, left + width - element.offsetWidth - 12))}px`;
      element.style.top = `${Math.max(top + 12, bottom - element.offsetHeight)}px`;
      const x = bounds
        ? bounds.left + bounds.width / 2
        : parseFloat(element.style.left);
      const y = bounds ? bounds.top + bounds.height / 2 : bottom + 16;
      element.style.transformOrigin = `${x - parseFloat(element.style.left)}px ${y - parseFloat(element.style.top)}px`;
    };
    position();
    move(true);
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !element.contains(event.target) &&
        !anchor?.current?.contains(event.target)
      )
        dismiss();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.isComposing) {
        event.preventDefault();
        event.stopPropagation();
        dismiss();
      }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape, true);
    window.addEventListener("resize", position);
    window.addEventListener("scroll", position, true);
    window.visualViewport?.addEventListener("resize", position);
    window.visualViewport?.addEventListener("scroll", position);
    const resize = new ResizeObserver(position);
    if (anchor?.current?.parentElement)
      resize.observe(anchor.current.parentElement);
    return () => {
      motion.current?.cancel();
      resize.disconnect();
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape, true);
      window.removeEventListener("resize", position);
      window.removeEventListener("scroll", position, true);
      window.visualViewport?.removeEventListener("resize", position);
      window.visualViewport?.removeEventListener("scroll", position);
    };
  }, [anchor, dismiss, move]);
  useLayoutEffect(() => {
    if (closing) dismiss();
    else if (exiting.current) move(true);
  }, [closing, dismiss, move]);
  return { dialog, exiting, dismiss };
}
