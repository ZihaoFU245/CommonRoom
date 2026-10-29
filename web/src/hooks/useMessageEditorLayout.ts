import { useEvent } from "./useEvent.ts";
import { useEffect, useLayoutEffect, useRef } from "preact/hooks";
import { editorScrollTop } from "../features/conversation/editorSizing.ts";
import { messageMotion } from "../features/conversation/timelineMotion.ts";

interface Context {
  active: boolean;
  text: string;
  anchorBottom: boolean;
}
interface BottomAnchor {
  viewport: HTMLElement;
  gap: number;
}
function conversationViewport(element: HTMLElement | null) {
  return element?.closest<HTMLElement>(".message-list") ?? null;
}

/** Size and animate the editor without moving the last message's footer. */
export function useMessageEditorLayout({
  active,
  text,
  anchorBottom,
}: Context) {
  const input = useRef<HTMLTextAreaElement>(null);
  const body = useRef<HTMLFormElement>(null);
  const animation = useRef<Animation | null>(null);
  const scrollFrame = useRef(0);
  const transition = useRef<{
    height: number;
    anchor: BottomAnchor | null;
  } | null>(null);

  function stopAnimation() {
    cancelAnimationFrame(scrollFrame.current);
    animation.current?.cancel();
    animation.current = null;
  }
  const fitInput = useEvent(() => {
    const element = input.current;
    if (!element) return;
    const form = element.form ?? element;
    const viewport = conversationViewport(element);
    const oldTop = viewport?.scrollTop ?? 0;
    const footer =
      anchorBottom && viewport
        ? (form.querySelector<HTMLElement>(":scope > div") ?? form)
        : null;
    const oldBottom = footer
      ? footer.getBoundingClientRect().bottom + oldTop
      : 0;
    stopAnimation();
    element.style.height = "0px";
    element.style.height = `${element.scrollHeight}px`;
    if (viewport) {
      viewport.scrollTop = footer
        ? editorScrollTop(
            oldTop,
            oldBottom,
            footer.getBoundingClientRect().bottom + viewport.scrollTop,
            viewport.scrollHeight - viewport.clientHeight,
          )
        : oldTop;
    }
  });
  useLayoutEffect(() => {
    if (active) fitInput();
  }, [active, text, anchorBottom, fitInput]);
  useLayoutEffect(() => {
    const element = input.current;
    if (!active || !element) return;
    let width = element.clientWidth;
    const observer = new ResizeObserver(() => {
      if (element.clientWidth !== width) {
        width = element.clientWidth;
        fitInput();
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [active, anchorBottom, fitInput]);

  function prepareTransition() {
    const form = body.current;
    const viewport = anchorBottom ? conversationViewport(form) : null;
    transition.current = form
      ? {
          height: form.getBoundingClientRect().height,
          anchor: viewport
            ? {
                viewport,
                gap:
                  viewport.scrollHeight -
                  viewport.clientHeight -
                  viewport.scrollTop,
              }
            : null,
        }
      : null;
    stopAnimation();
  }
  useLayoutEffect(() => {
    const form = body.current;
    const pending = transition.current;
    transition.current = null;
    if (!form || !pending) return;
    const pinBottom = () => {
      if (pending.anchor) {
        const { viewport, gap } = pending.anchor;
        viewport.scrollTop = Math.max(
          0,
          viewport.scrollHeight - viewport.clientHeight - gap,
        );
      }
    };
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      pinBottom();
      return;
    }
    // Keep the body mounted; animating removed inputs would reinsert them.
    const current = form.animate(
      [
        { height: `${pending.height}px`, overflow: "hidden" },
        {
          height: `${form.getBoundingClientRect().height}px`,
          overflow: "hidden",
        },
      ],
      messageMotion,
    );
    animation.current = current;
    if (pending.anchor) {
      const follow = () => {
        pinBottom();
        if (current.playState === "running")
          scrollFrame.current = requestAnimationFrame(follow);
      };
      follow();
    }
    current.addEventListener(
      "finish",
      () => {
        if (animation.current !== current) return;
        stopAnimation();
        pinBottom();
      },
      { once: true },
    );
  }, [active]);
  useEffect(() => stopAnimation, []);
  useEffect(() => {
    if (active) input.current?.focus();
  }, [active]);

  return { input, body, prepareTransition };
}
