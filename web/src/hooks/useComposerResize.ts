import type { JSX, RefObject } from "preact";
import { useRef, useState } from "preact/hooks";
export function useComposerResize(
  chatFontSize: number,
  input: RefObject<HTMLTextAreaElement>,
) {
  const [composerHeight, setComposerHeight] = useState(24);
  const minimumComposerHeight = Math.ceil(chatFontSize * 1.5 + 6);
  const actualComposerHeight = Math.max(minimumComposerHeight, composerHeight);
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
