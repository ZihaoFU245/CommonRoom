import type { ComponentChildren } from "preact";
import { useContentResizeMotion } from "../hooks/useContentResizeMotion.ts";
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
  const resize = useContentResizeMotion();
  function prepare(expanding: boolean) {
    const shellHeight = resize.panel.current?.getBoundingClientRect().height;
    const naturalHeight =
      resize.content.current?.getBoundingClientRect().height;
    const previewHeight = panel.current?.getBoundingClientRect().height;
    // Transfer an unfinished media reveal to explicit folding without exposing
    // the full natural height between the two animations.
    const visibleHeight =
      shellHeight !== undefined &&
      naturalHeight !== undefined &&
      previewHeight !== undefined
        ? Math.max(0, shellHeight - (naturalHeight - previewHeight))
        : undefined;
    resize.stop();
    prepareTransition(expanding, visibleHeight);
  }
  return (
    <div ref={resize.panel} class="collapsible-message">
      <div ref={resize.content} class="collapsible-message-content">
        <div
          id={id}
          ref={panel}
          class={`message-preview${expanded ? " expanded" : ""}${overflows && !expanded ? " folded" : ""}`}
          onFocusCapture={() => {
            // Keyboard focus must never disappear into the clipped part of a message.
            if (overflows && !expanded) {
              prepare(true);
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
              prepare(!expanded);
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
    </div>
  );
}
