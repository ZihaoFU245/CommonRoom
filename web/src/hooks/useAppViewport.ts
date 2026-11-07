import { useLayoutEffect } from "preact/hooks";

/** Follow the keyboard-sized viewport; pinch zoom must not resize the app. */
export function useAppViewport() {
  useLayoutEffect(() => {
    const visual = window.visualViewport;
    let frame = 0;
    const update = () => {
      if (visual && visual.scale !== 1) return;
      document.documentElement.style.setProperty(
        "--app-height",
        `${visual?.height ?? window.innerHeight}px`,
      );
    };
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(update);
    };
    update();
    visual?.addEventListener("resize", schedule);
    window.addEventListener("resize", schedule);
    return () => {
      cancelAnimationFrame(frame);
      visual?.removeEventListener("resize", schedule);
      window.removeEventListener("resize", schedule);
      document.documentElement.style.removeProperty("--app-height");
    };
  }, []);
}
