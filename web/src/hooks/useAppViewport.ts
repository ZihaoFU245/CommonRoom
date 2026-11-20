import { useLayoutEffect } from "preact/hooks";

/** Track keyboard resize and pan without changing the layout during pinch zoom. */
export function useAppViewport() {
  useLayoutEffect(() => {
    const visual = window.visualViewport;
    const style = document.documentElement.style;
    let frame = 0;
    const update = () => {
      if (visual && visual.scale !== 1) return;
      style.setProperty(
        "--app-height",
        `${visual?.height ?? window.innerHeight}px`,
      );
      style.setProperty("--app-top", `${visual?.offsetTop ?? 0}px`);
    };
    const schedule = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(update);
    };
    update();
    visual?.addEventListener("resize", schedule);
    visual?.addEventListener("scroll", schedule);
    window.addEventListener("resize", schedule);
    return () => {
      cancelAnimationFrame(frame);
      visual?.removeEventListener("resize", schedule);
      visual?.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      style.removeProperty("--app-height");
      style.removeProperty("--app-top");
    };
  }, []);
}
