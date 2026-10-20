import { HtmlSource } from "./HtmlSource.tsx";
import type { ComponentChildren } from "preact";
import { useContext, useEffect, useLayoutEffect, useRef } from "preact/hooks";
import { HtmlGateContext, type HtmlGateMode } from "./HtmlGateContext.ts";
import { messageMotion } from "../features/conversation/timelineMotion.ts";

export interface HtmlCoverProps {
  text: string;
  messageKey: string;
  choiceKey?: string;
  consentText?: string;
  children?: ComponentChildren;
  label?: ComponentChildren;
  allowRender?: boolean;
  textStatus?: ComponentChildren;
  onModeChange?: (mode: HtmlGateMode) => void;
}
export function HtmlCover({
  text,
  messageKey,
  children,
  label = "HTML content",
  choiceKey = messageKey,
  consentText = text,
  allowRender = true,
  textStatus,
  onModeChange,
}: HtmlCoverProps) {
  const gates = useContext(HtmlGateContext);
  const choice = gates?.choices.get(choiceKey);
  const selected = choice?.text === consentText ? choice.mode : null;
  const mode = selected === "render" && !allowRender ? null : selected;
  const modeChanged = useRef(onModeChange);
  modeChanged.current = onModeChange;
  useEffect(() => {
    modeChanged.current?.(mode);
  }, [mode]);
  const panel = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const scrollFrame = useRef(0);
  const anchor = useRef<{
    viewport: HTMLElement;
    bottom: number;
    followBothWays: boolean;
  } | null>(null);
  const previousHeight = useRef<number | null>(null);
  const animation = useRef<Animation | null>(null);
  function pinContent() {
    const element = panel.current;
    const target = anchor.current;
    if (!element || !target) return;
    const delta = element.getBoundingClientRect().bottom - target.bottom;
    if (target.followBothWays || delta > 0) target.viewport.scrollTop += delta;
  }
  function stopAnimation() {
    cancelAnimationFrame(scrollFrame.current);
    animation.current?.cancel();
    animation.current = null;
  }
  function select(next: HtmlGateMode) {
    const element = panel.current;
    const viewport = element?.closest<HTMLElement>(".message-list");
    if (element && viewport) {
      const bounds = viewport.getBoundingClientRect();
      const atBottom =
        viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop < 24;
      anchor.current = {
        viewport,
        bottom: atBottom
          ? Math.min(element.getBoundingClientRect().bottom, bounds.bottom - 8)
          : bounds.bottom - 8,
        followBothWays: atBottom,
      };
    }
    previousHeight.current = panel.current?.getBoundingClientRect().height ?? 0;
    stopAnimation();
    gates?.change(
      choiceKey,
      messageKey,
      consentText,
      mode === next ? null : next,
    );
  }
  useLayoutEffect(() => {
    const element = panel.current;
    const height = previousHeight.current;
    previousHeight.current = null;
    if (
      !element ||
      height === null ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      pinContent();
      return;
    }
    const current = element.animate(
      [
        { height: `${height}px`, overflow: "hidden" },
        {
          height: `${element.getBoundingClientRect().height}px`,
          overflow: "hidden",
        },
      ],
      messageMotion,
    );
    animation.current = current;
    const follow = () => {
      pinContent();
      if (current.playState === "running")
        scrollFrame.current = requestAnimationFrame(follow);
    };
    follow();
    current.addEventListener(
      "finish",
      () => {
        if (animation.current !== current) return;
        stopAnimation();
        pinContent();
      },
      { once: true },
    );
  }, [mode]);
  // The frame module and its reported height can arrive after the click animation.
  // Observe natural content, not the animated panel, to avoid resize feedback.
  useLayoutEffect(() => {
    const element = content.current;
    const viewport = panel.current?.closest<HTMLElement>(".message-list");
    if (!mode || !element || !viewport) return;
    const observer = new ResizeObserver(pinContent);
    observer.observe(element);
    const release = () => {
      anchor.current = null;
    };
    const events = ["wheel", "touchstart", "pointerdown", "keydown"] as const;
    for (const event of events)
      viewport.addEventListener(event, release, { passive: true });
    return () => {
      observer.disconnect();
      for (const event of events) viewport.removeEventListener(event, release);
    };
  }, [mode]);
  useEffect(() => stopAnimation, []);
  return (
    <div class="html-gate">
      <div class="html-gate-controls" role="group" aria-label="HTML content">
        <span>{label}</span>
        <div class="html-gate-actions">
          <button
            type="button"
            aria-expanded={mode === "text"}
            onClick={() => select("text")}
          >
            Show text
          </button>
          {allowRender && (
            <button
              type="button"
              class="primary"
              aria-expanded={mode === "render"}
              onClick={() => select("render")}
            >
              Render HTML
            </button>
          )}
        </div>
      </div>
      <div ref={panel} class="html-gate-panel">
        <div ref={content} class="html-gate-content">
          {mode === "text" && (textStatus ?? <HtmlSource text={text} />)}
          {mode === "render" && (
            <div class="html-gate-preview">
              {children ?? <p role="status">Loading HTML preview…</p>}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
