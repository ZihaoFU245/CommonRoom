import { HtmlSource } from "./HtmlSource.tsx";
import type { ComponentChildren } from "preact";
import { useContext, useEffect, useRef } from "preact/hooks";
import { HtmlGateContext, type HtmlGateMode } from "./HtmlGateContext.ts";

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
  function select(next: HtmlGateMode) {
    gates?.change(
      choiceKey,
      messageKey,
      consentText,
      mode === next ? null : next,
    );
  }
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
      <div class="html-gate-panel">
        <div class="html-gate-content">
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
