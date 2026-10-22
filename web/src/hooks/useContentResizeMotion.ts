import { createContext, type RefObject } from "preact";
import { useContext, useLayoutEffect, useRef } from "preact/hooks";
import { messageMotion } from "../features/conversation/timelineMotion.ts";
import { observedBorderBox } from "../features/conversation/observedBorderBox.ts";

export const ContentMotionContext = createContext<RefObject<boolean> | null>(
  null,
);

/** Animate delayed layout changes once, outside the natural content and fold control. */
export function useContentResizeMotion() {
  const atBottom = useContext(ContentMotionContext);
  const panel = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const animation = useRef<Animation | null>(null);
  const frame = useRef(0);
  const pinned = useRef(false);
  function stop() {
    cancelAnimationFrame(frame.current);
    animation.current?.cancel();
    animation.current = null;
    delete panel.current?.dataset.contentResizing;
  }
  useLayoutEffect(() => {
    const clip = panel.current;
    const natural = content.current;
    if (!clip || !natural) return;
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    let previous: { width: number; height: number } =
      natural.getBoundingClientRect();
    let explicitResize = false;
    const observer = new ResizeObserver((records) => {
      const record = records[0];
      if (!record) return;
      const next = observedBorderBox(record);
      const before = previous;
      previous = next;
      const folding = natural.querySelector("[data-message-resizing]") !== null;
      // The final fold resize can arrive after its animation removes the marker.
      const finishingFold = explicitResize && !folding;
      explicitResize = folding;
      // Explicit folding owns its own animation and footer pin. Width changes
      // should reflow immediately, rather than replaying every mounted message.
      if (
        folding ||
        finishingFold ||
        Math.abs(before.width - next.width) > 0.5 ||
        preference.matches
      ) {
        stop();
        return;
      }
      if (Math.abs(before.height - next.height) <= 0.5) return;
      const viewport = clip.closest<HTMLElement>(".message-list");
      const bounds = clip.getBoundingClientRect();
      const visible = viewport?.getBoundingClientRect();
      if (
        visible &&
        (bounds.bottom < visible.top || bounds.top > visible.bottom)
      ) {
        stop();
        return;
      }
      // Retarget from the currently painted size when several assets finish
      // together. Observe the inner content so our height cannot feed back.
      const height = animation.current ? bounds.height : before.height;
      const followBottom = animation.current
        ? pinned.current
        : (atBottom?.current ??
          (!!viewport &&
            viewport.scrollHeight -
              viewport.clientHeight -
              viewport.scrollTop -
              (next.height - before.height) <
              24));
      stop();
      pinned.current = followBottom;
      if (followBottom) clip.dataset.contentResizing = "";
      const current = clip.animate(
        [
          { height: `${height}px`, overflow: "clip" },
          { height: `${next.height}px`, overflow: "clip" },
        ],
        { ...messageMotion, fill: "both" },
      );
      animation.current = current;
      const follow = () => {
        if (pinned.current && viewport)
          viewport.scrollTop = viewport.scrollHeight - viewport.clientHeight;
        if (pinned.current && viewport && animation.current === current)
          frame.current = requestAnimationFrame(follow);
      };
      follow();
      current.addEventListener(
        "finish",
        () => {
          if (animation.current !== current) return;
          follow();
          stop();
        },
        { once: true },
      );
      // Virtual row measurements follow the animated height, preserving either
      // the chat bottom or the reader's existing message anchor on each frame.
    });
    observer.observe(natural, { box: "border-box" });
    preference.addEventListener("change", stop);
    const viewport = clip.closest<HTMLElement>(".message-list");
    const release = () => {
      pinned.current = false;
      cancelAnimationFrame(frame.current);
      delete clip.dataset.contentResizing;
    };
    const events = ["wheel", "touchstart", "pointerdown", "keydown"] as const;
    for (const event of events)
      viewport?.addEventListener(event, release, { passive: true });
    return () => {
      observer.disconnect();
      preference.removeEventListener("change", stop);
      for (const event of events) viewport?.removeEventListener(event, release);
      stop();
    };
  }, [atBottom]);
  return { panel, content, stop };
}
