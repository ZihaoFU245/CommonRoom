import { useEffect, useRef } from "preact/hooks";
import autoAnimate from "@formkit/auto-animate";
import { controlMotion } from "../features/preferences/motion.ts";

/** Keep notice motion separate from the virtual message rows. */
export function useHistoryNoticeMotion() {
  const container = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    const animation = autoAnimate(element, (child, action) => {
      // Settle layout once. Animating height would resize the virtual viewport
      // every frame and make scroll anchoring compete with the notice motion.
      const frames: Keyframe[] =
        action === "remain"
          ? [{ transform: "none" }]
          : action === "remove"
            ? [
                { transform: "translateY(0)" },
                { transform: "translateY(-100%)" },
              ]
            : [
                { transform: "translateY(-100%)" },
                { transform: "translateY(0)" },
              ];
      return new KeyframeEffect(child, frames, {
        ...controlMotion,
        duration:
          preference.matches || action === "remain"
            ? 0
            : controlMotion.duration,
      });
    });
    const update = () => {
      if (preference.matches) {
        animation.disable();
        for (const active of element.getAnimations({ subtree: true }))
          active.finish();
      } else animation.enable();
    };
    update();
    preference.addEventListener("change", update);
    return () => {
      preference.removeEventListener("change", update);
      animation.destroy?.();
    };
  }, []);
  return container;
}
