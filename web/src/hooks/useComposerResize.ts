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
  const measure = useRef<(() => void) | null>(null);
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
    measure.current = () => {
      const style = getComputedStyle(element);
      mirror.style.width = `${element.getBoundingClientRect().width}px`;
      mirror.style.font = style.font;
      mirror.style.fontFamily = style.fontFamily;
      mirror.style.fontSize = style.fontSize;
      mirror.style.fontWeight = style.fontWeight;
      mirror.style.lineHeight = style.lineHeight;
      mirror.style.letterSpacing = style.letterSpacing;
      mirror.style.padding = style.padding;
      mirror.style.border = style.border;
      mirror.style.boxSizing = style.boxSizing;
      mirror.style.wordBreak = style.wordBreak;
      mirror.style.overflowWrap = style.overflowWrap;
      mirror.wrap = element.wrap;
      mirror.value = element.value;
      setContentHeight(mirror.scrollHeight);
    };
    measure.current();
    let width = element.clientWidth;
    const observer = new ResizeObserver(() => {
      if (width !== element.clientWidth) {
        width = element.clientWidth;
        measure.current?.();
      }
    });
    observer.observe(element);
    const resize = () => {
      setViewportHeight(window.innerHeight);
      measure.current?.();
    };
    window.addEventListener("resize", resize);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", resize);
      measure.current = null;
      mirror.remove();
    };
  }, [input]);
  useLayoutEffect(() => {
    measure.current?.();
  }, [draft, chatFontSize]);
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
