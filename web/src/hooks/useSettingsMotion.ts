import { controlMotion } from "../features/preferences/motion.ts";
import autoAnimate, { type AutoAnimationPlugin } from "@formkit/auto-animate";
import { useEffect, useRef } from "preact/hooks";

/** Short settings transitions, including changes to the reduced-motion preference. */
export function useSettingsMotion<T extends HTMLElement>(
  plugin?: AutoAnimationPlugin,
) {
  const container = useRef<T>(null);
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    let animation: ReturnType<typeof autoAnimate> | undefined;
    const sync = () => {
      animation?.destroy?.();
      animation = reduced.matches
        ? undefined
        : autoAnimate(element, plugin ?? controlMotion);
    };
    sync();
    reduced.addEventListener("change", sync);
    return () => {
      reduced.removeEventListener("change", sync);
      animation?.destroy?.();
    };
  }, [plugin]);
  return container;
}
