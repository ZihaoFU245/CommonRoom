import type { ComponentChildren } from "preact";
import { useId } from "preact/hooks";
import { useMessageExpansionMotion } from "../hooks/useMessageExpansionMotion.ts";

/** Fold the rendered body, including fenced code and asynchronously loaded media. */
export function CollapsibleMessage({
  children,
  expanded,
  onExpandedChange,
}: {
  children: ComponentChildren;
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
}) {
  const id = useId();
  const { panel, content, toggle, overflows, prepareTransition } =
    useMessageExpansionMotion();
  return (
    <div class="collapsible-message">
      <div
        id={id}
        ref={panel}
        class={`message-preview${expanded ? " expanded" : ""}${overflows && !expanded ? " folded" : ""}`}
        onFocusCapture={() => {
          // Keyboard focus must never disappear into the clipped part of a message.
          if (overflows && !expanded) {
            prepareTransition(true);
            onExpandedChange(true);
          }
        }}
      >
        <div ref={content} class="message-preview-content">
          {children}
        </div>
      </div>
      {overflows && (
        <button
          ref={toggle}
          type="button"
          class="message-expand-toggle"
          aria-expanded={expanded}
          aria-controls={id}
          onClick={() => {
            prepareTransition(!expanded);
            onExpandedChange(!expanded);
          }}
        >
          <span>{expanded ? "Show less" : "Show more"}</span>
          <svg aria-hidden="true" width="12" height="12" viewBox="0 0 16 16">
            <path
              d={expanded ? "M8 13V3m-4 4 4-4 4 4" : "M8 3v10m-4-4 4 4 4-4"}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </button>
      )}
    </div>
  );
}
