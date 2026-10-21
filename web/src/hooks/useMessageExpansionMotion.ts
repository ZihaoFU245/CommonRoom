import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { messageExpansionMotion } from "../features/conversation/messageExpansion.ts";

/** Start before the expansion state changes, so virtual rows measure the animation. */
export function useMessageExpansionMotion() {
  const panel = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const toggle = useRef<HTMLButtonElement>(null);
  const stop = useRef<(() => void) | null>(null);
  const [overflows, setOverflows] = useState(false);
  useLayoutEffect(() => {
    const element = content.current;
    const clip = panel.current;
    if (!element || !clip) return;
    const measure = () => {
      const limit = parseFloat(
        getComputedStyle(clip).getPropertyValue("--message-preview-height"),
      );
      setOverflows(element.getBoundingClientRect().height > limit + 1);
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  useEffect(() => () => stop.current?.(), []);

  function prepareTransition(expanding: boolean, visibleHeight?: number) {
    const clip = panel.current;
    const natural = content.current;
    if (!clip || !natural) return;
    // Capture an interrupted animation's current size before cancelling it.
    const height = visibleHeight ?? clip.getBoundingClientRect().height;
    const viewport = clip.closest<HTMLElement>(".message-list");
    const bottom = toggle.current?.getBoundingClientRect().bottom;
    stop.current?.();
    clip.dataset.messageResizing = "";
    let pinned = !expanding && viewport !== null && bottom !== undefined;
    let frame = 0;
    let animation: Animation | null = null;
    const pin = () => {
      const button = toggle.current;
      if (pinned && viewport && button && bottom !== undefined)
        viewport.scrollTop += button.getBoundingClientRect().bottom - bottom;
    };
    const release = () => {
      pinned = false;
    };
    const events = ["wheel", "touchstart", "pointerdown", "keydown"] as const;
    for (const event of events)
      viewport?.addEventListener(event, release, { passive: true });
    const cleanup = () => {
      cancelAnimationFrame(frame);
      animation?.cancel();
      delete clip.dataset.messageResizing;
      for (const event of events) viewport?.removeEventListener(event, release);
      if (stop.current === cleanup) stop.current = null;
    };
    stop.current = cleanup;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      // The next frame sees the committed height, with no animation.
      frame = requestAnimationFrame(() => {
        pin();
        cleanup();
      });
      return;
    }
    const limit = parseFloat(
      getComputedStyle(clip).getPropertyValue("--message-preview-height"),
    );
    const target = expanding
      ? natural.getBoundingClientRect().height
      : Math.min(limit, natural.getBoundingClientRect().height);
    const viewportHeight = viewport
      ? viewport.getBoundingClientRect().bottom -
        clip.getBoundingClientRect().top
      : target;
    const motion = messageExpansionMotion(height, target, viewportHeight);
    animation = clip.animate(
      motion.frames.map((keyframe) => ({
        ...keyframe,
        maxHeight: "none",
        overflow: "hidden",
      })),
      { duration: motion.duration, easing: motion.easing, fill: "both" },
    );
    const current = animation;
    const follow = () => {
      pin();
      if (current.playState === "running")
        frame = requestAnimationFrame(follow);
    };
    follow();
    current.addEventListener(
      "finish",
      () => {
        if (stop.current !== cleanup) return;
        pin();
        cleanup();
        pin();
      },
      { once: true },
    );
  }
  return { panel, content, toggle, overflows, prepareTransition };
}
