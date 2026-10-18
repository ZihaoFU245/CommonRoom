import type { JSX, RefObject } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
export function useComposerResize(
  chatFontSize: number,
  input: RefObject<HTMLTextAreaElement>,
  draft: string,
) {
  const [composerHeight, setComposerHeight] = useState(24);
  const [contentHeight, setContentHeight] = useState(24);
  const [viewportHeight, setViewportHeight] = useState(
    () => window.innerHeight,
  );
  const minimumComposerHeight = Math.ceil(chatFontSize * 1.5 + 6);
  const maximumComposerHeight = Math.max(
    minimumComposerHeight,
    Math.min(240, viewportHeight * 0.4),
  );
  const actualComposerHeight = Math.min(
    maximumComposerHeight,
    Math.max(minimumComposerHeight, composerHeight, contentHeight),
  );
  const measure = useRef<((refreshStyle?: boolean) => void) | null>(null);
  useLayoutEffect(() => {
    const element = input.current;
    if (!element) return;
    const mirror = document.createElement("textarea");
    mirror.name = "message-size-mirror";
    mirror.tabIndex = -1;
    mirror.setAttribute("aria-hidden", "true");
    mirror.style.cssText =
      "position:fixed;left:-10000px;top:0;height:0;min-height:0;max-height:none;visibility:hidden;pointer-events:none;overflow:hidden;";
    document.body.appendChild(mirror);
    let frame = 0;
    let styleDirty = true;
    const measureNow = () => {
      if (styleDirty) {
        const style = getComputedStyle(element);
        mirror.style.width = `${element.getBoundingClientRect().width}px`;
        for (const property of [
          "font",
          "fontFamily",
          "fontSize",
          "fontWeight",
          "lineHeight",
          "letterSpacing",
          "padding",
          "border",
          "boxSizing",
          "wordBreak",
          "overflowWrap",
        ] as const)
          mirror.style[property] = style[property];
        mirror.wrap = element.wrap;
        styleDirty = false;
      }
      mirror.value = element.value;
      setContentHeight(mirror.scrollHeight);
    };
    measure.current = (refreshStyle = false) => {
      styleDirty ||= refreshStyle;
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        measureNow();
      });
    };
    measureNow();
    let width = element.clientWidth;
    const observer = new ResizeObserver(() => {
      if (width !== element.clientWidth) {
        width = element.clientWidth;
        measure.current?.(true);
      }
    });
    observer.observe(element);
    const resize = () => {
      setViewportHeight(window.innerHeight);
      measure.current?.(true);
    };
    const styles = new MutationObserver(() => measure.current?.(true));
    styles.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["style", "class"],
    });
    document.fonts?.addEventListener("loadingdone", resize);
    window.addEventListener("resize", resize);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      styles.disconnect();
      document.fonts?.removeEventListener("loadingdone", resize);
      window.removeEventListener("resize", resize);
      measure.current = null;
      mirror.remove();
    };
  }, [input]);
  useLayoutEffect(() => {
    measure.current?.();
  }, [draft]);
  useLayoutEffect(() => {
    measure.current?.(true);
  }, [chatFontSize]);
  useEffect(() => {
    if (!draft) setComposerHeight(minimumComposerHeight);
  }, [draft, minimumComposerHeight]);
  const resizeStart = useRef<{ y: number; height: number } | null>(null);
  function resizeComposer(height: number) {
    setComposerHeight(
      Math.max(
        minimumComposerHeight,
        Math.min(240, window.innerHeight * 0.4, height),
      ),
    );
  }
  function startResize(event: JSX.TargetedPointerEvent<HTMLDivElement>) {
    if (event.button !== 0) return;
    event.preventDefault();
    resizeStart.current = {
      y: event.clientY,
      height: input.current?.clientHeight || minimumComposerHeight,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  }
  function moveResize(event: JSX.TargetedPointerEvent<HTMLDivElement>) {
    if (resizeStart.current)
      resizeComposer(
        resizeStart.current.height + resizeStart.current.y - event.clientY,
      );
  }
  function endResize() {
    resizeStart.current = null;
  }
  return {
    minimumComposerHeight,
    actualComposerHeight,
    startResize,
    moveResize,
    endResize,
    resizeComposer,
  };
}
