import { useEvent } from "./useEvent.ts";
import type { RefObject } from "preact";
import { useEffect, useLayoutEffect, useRef } from "preact/hooks";
import type { TimelineEntry } from "../api/protocol.ts";
import {
  messageMotion,
  timelineChanges,
} from "../features/conversation/timelineMotion.ts";

interface Row {
  element: HTMLElement;
  top: number;
  left: number;
  width: number;
  height: number;
  bodyOffset: number | null;
  meta: Chrome | null;
  day: Chrome | null;
}
interface Chrome {
  element: HTMLElement;
  top: number;
  left: number;
  width: number;
  height: number;
}
function chromeSnapshot(
  element: HTMLElement | null,
  row: DOMRect,
): Chrome | null {
  if (!element) return null;
  const rect = element.getBoundingClientRect();
  return {
    element,
    top: rect.top - row.top,
    left: rect.left - row.left,
    width: rect.width,
    height: rect.height,
  };
}

/** Animate visual positions, leaving virtual row sizes at their final values. */
export function useTimelineMotion(
  entries: TimelineEntry[],
  scope: string,
  revision: number,
  ready: boolean,
  container: RefObject<HTMLElement>,
  atBottom: RefObject<boolean>,
  layout: {
    start: number;
    end: number;
    layoutVersion: number;
    rowMeasurements?: {
      rects: ReadonlyMap<HTMLElement, DOMRect>;
      scrollTop: number;
    };
  },
) {
  const baseline = useRef<{
    scope: string;
    revision: number;
    entries: TimelineEntry[];
  } | null>(null);
  const rows = useRef(new Map<string, Row>());
  const capturedScrollTop = useRef(0);
  const animations = useRef(new Set<Animation>());
  const ghosts = useRef(new Set<HTMLElement>());
  const positions = useRef(
    new Map<HTMLElement, { animation: Animation; shift: number }>(),
  );
  function displacement(element: HTMLElement) {
    const motion = positions.current.get(element);
    if (!motion) return 0;
    const progress = motion.animation.effect?.getComputedTiming().progress ?? 1;
    return motion.shift * (1 - progress);
  }
  function visualRows() {
    return new Map(
      [...rows.current].map(([key, row]) => {
        const body = row.element.querySelector<HTMLElement>(
          ".message-body, .message-editor",
        );
        return [
          key,
          {
            ...row,
            top: row.top + displacement(row.element),
            bodyOffset:
              row.bodyOffset === null
                ? null
                : row.bodyOffset + (body ? displacement(body) : 0),
          },
        ];
      }),
    );
  }
  const scheduled = useRef(0);
  const pending = useRef<{
    entries: TimelineEntry[];
    changes: ReturnType<typeof timelineChanges>;
    before: Map<string, Row>;
    pinBottom: boolean;
  } | null>(null);

  const capture = useEvent(
    (measured?: {
      rects: ReadonlyMap<HTMLElement, DOMRect>;
      scrollTop: number;
    }) => {
      const viewport = container.current;
      if (!viewport || globalThis.document?.hidden) return;
      const bounds = viewport.getBoundingClientRect();
      capturedScrollTop.current = viewport.scrollTop;
      rows.current = new Map(
        [
          ...viewport.querySelectorAll<HTMLElement>(
            ".timeline-rows > [data-virtual-key]",
          ),
        ].map((element) => {
          const rect =
            (measured?.scrollTop === viewport.scrollTop
              ? measured.rects.get(element)
              : undefined) ?? element.getBoundingClientRect();
          const body = element.querySelector<HTMLElement>(
            ".message-body, .message-editor",
          );
          return [
            element.dataset.virtualKey ?? "",
            {
              element,
              top: rect.top - bounds.top - displacement(element),
              left: rect.left - bounds.left,
              width: rect.width,
              height: rect.height,
              bodyOffset: body
                ? body.getBoundingClientRect().top -
                  rect.top -
                  displacement(body)
                : null,
              meta: chromeSnapshot(
                element.querySelector<HTMLElement>(".message-meta"),
                rect,
              ),
              day: chromeSnapshot(
                element.querySelector<HTMLElement>(".message-day"),
                rect,
              ),
            },
          ];
        }),
      );
    },
  );
  const stop = useEvent(() => {
    for (const animation of animations.current) animation.cancel();
    animations.current.clear();
    for (const ghost of ghosts.current) ghost.remove();
    ghosts.current.clear();
    clearClip();
  });
  function clearClip() {
    const content =
      container.current?.querySelector<HTMLElement>(".timeline-rows");
    content?.classList.remove("is-animating");
    content?.style.removeProperty("min-height");
  }
  function animate(
    element: HTMLElement,
    keyframes: Keyframe[],
    remove = false,
  ) {
    if (!animations.current.size) {
      const viewport = container.current;
      const content = viewport?.querySelector<HTMLElement>(".timeline-rows");
      if (viewport && content) {
        const style = getComputedStyle(viewport);
        // Short histories need the empty viewport space for rows moving upward.
        // This changes the clip boundary without enlarging the scroll range.
        content.style.minHeight = `${Math.max(0, viewport.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom))}px`;
        content.classList.add("is-animating");
      }
    }
    const animation = element.animate(keyframes, messageMotion);
    animations.current.add(animation);
    const transform = keyframes[0]?.transform;
    if (typeof transform === "string" && transform.startsWith("translateY(")) {
      positions.current.set(element, {
        animation,
        shift: parseFloat(transform.slice(11)),
      });
    }
    const finish = () => {
      animations.current.delete(animation);
      if (positions.current.get(element)?.animation === animation)
        positions.current.delete(element);
      if (remove) {
        element.remove();
        ghosts.current.delete(element);
      }
      if (!animations.current.size) clearClip();
    };
    animation.addEventListener("finish", finish, { once: true });
    animation.addEventListener("cancel", finish, { once: true });
  }
  function exit(row: Row, chrome?: Chrome) {
    const viewport = container.current;
    if (!viewport) return;
    const top = row.top + (chrome?.top ?? 0);
    const height = chrome?.height ?? row.height;
    if (top + height < 0 || top >= viewport.clientHeight) return;
    const copy = (chrome?.element ?? row.element).cloneNode(true);
    if (!(copy instanceof HTMLElement)) return;
    const clone = chrome ? document.createElement("div") : copy;
    if (chrome) {
      clone.classList.add("chat-message");
      clone.style.padding = "0";
      clone.style.margin = "0";
      clone.append(copy);
    }
    clone.removeAttribute("data-virtual-key");
    for (const element of clone.querySelectorAll("[id], [data-sequence]")) {
      element.removeAttribute("id");
      element.removeAttribute("data-sequence");
    }
    clone.setAttribute("aria-hidden", "true");
    clone.inert = true;
    clone.classList.add("message-exit");
    // Exit copies stay independent of collapsing rows, within the scroll extent.
    Object.assign(clone.style, {
      top: `${top + viewport.scrollTop}px`,
      left: `${row.left + (chrome?.left ?? 0)}px`,
      width: `${chrome?.width ?? row.width}px`,
      height: `${Math.min(height, viewport.clientHeight - top)}px`,
    });
    viewport.append(clone);
    ghosts.current.add(clone);
    animate(clone, [{ opacity: 1 }, { opacity: 0 }], true);
  }

  const updateMotion = useEvent(() => {
    const old = baseline.current;
    if (old?.scope !== scope) {
      stop();
      pending.current = null;
      rows.current.clear();
      baseline.current = null;
    }
    if (ready) {
      if (old?.scope === scope && old.revision !== revision) {
        const previous = pending.current;
        const source = previous?.entries ?? old.entries;
        const changes = timelineChanges(source, entries);
        if (changes.added.size || changes.removed.size || changes.edited.size) {
          pending.current = {
            entries: source,
            changes,
            before: previous?.before ?? visualRows(),
            pinBottom: previous?.pinBottom ?? atBottom.current === true,
          };
        } else pending.current = null;
      }
      baseline.current = { scope, revision, entries };
    }
    // Settle layout effects and virtual measurements before the browser paints.
    const ticket = ++scheduled.current;
    queueMicrotask(() => {
      if (ticket !== scheduled.current) return;
      const viewport = container.current;
      const update = pending.current;
      pending.current = null;
      if (!viewport || !update) {
        capture(layout.rowMeasurements);
        return;
      }
      // Stopping a running transform changes row geometry; otherwise reuse
      // the virtualizer's measurements from this commit when scroll is unchanged.
      const measured = animations.current.size
        ? undefined
        : layout.rowMeasurements;
      stop();
      if (update.pinBottom)
        viewport.scrollTop = viewport.scrollHeight - viewport.clientHeight;
      capture(measured);
      if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
      for (const [key, row] of rows.current) {
        const before = update.before.get(key);
        if (before) {
          const shift = before.top - row.top;
          if (Math.abs(shift) > 0.5)
            animate(row.element, [
              { transform: `translateY(${shift}px)` },
              { transform: "translateY(0)" },
            ]);
          // A surviving grouped row can gain/lose a header or date divider.
          // Preserve the text's position as well as the row's outer position.
          const body = row.element.querySelector<HTMLElement>(
            ".message-body, .message-editor",
          );
          if (body && before.bodyOffset !== null && row.bodyOffset !== null) {
            const bodyShift = before.bodyOffset - row.bodyOffset;
            if (Math.abs(bodyShift) > 0.5)
              animate(body, [
                { transform: `translateY(${bodyShift}px)` },
                { transform: "translateY(0)" },
              ]);
          }
          for (const part of ["meta", "day"] as const) {
            const oldChrome = before[part];
            const nextChrome = row[part];
            if (!oldChrome && nextChrome)
              animate(nextChrome.element, [{ opacity: 0 }, { opacity: 1 }]);
            else if (oldChrome && !nextChrome) exit(before, oldChrome);
          }
        } else if (update.changes.added.has(key)) {
          animate(row.element, [
            {
              transform: `translateY(${Math.min(row.height, viewport.clientHeight)}px)`,
              opacity: 0,
            },
            { transform: "translateY(0)", opacity: 1 },
          ]);
        }
        if (update.changes.edited.has(key)) {
          const body = row.element.querySelector<HTMLElement>(".message-body");
          if (body) animate(body, [{ opacity: 0.4 }, { opacity: 1 }]);
        }
      }
      for (const key of update.changes.removed) {
        const row = update.before.get(key);
        if (row) exit(row);
      }
      // Final geometry was captured once before starting the animations.
    });
  });
  useLayoutEffect(() => {
    updateMotion();
  }, [
    updateMotion,
    entries,
    scope,
    revision,
    ready,
    layout.start,
    layout.end,
    layout.layoutVersion,
  ]);
  const invalidate = useEvent(() => {
    scheduled.current++;
  });
  useEffect(() => {
    const viewport = container.current;
    if (!viewport) return;
    const scroll = () => {
      if (pending.current) return;
      const delta = viewport.scrollTop - capturedScrollTop.current;
      capturedScrollTop.current = viewport.scrollTop;
      // Scrolling shifts cached coordinates; it does not need DOM measurements.
      for (const row of rows.current.values()) row.top -= delta;
    };
    const interrupt = () => {
      // Ordinary scrolling already shifts cached positions in the scroll handler.
      if (!pending.current && !animations.current.size) return;
      scheduled.current++;
      stop();
      pending.current = null;
      capture();
    };
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    const resize = new ResizeObserver(() => {
      if (!pending.current) capture();
    });
    resize.observe(viewport);
    viewport.addEventListener("scroll", scroll, { passive: true });
    const inputs = ["wheel", "touchstart", "pointerdown", "keydown"] as const;
    for (const input of inputs)
      viewport.addEventListener(input, interrupt, { passive: true });
    preference.addEventListener("change", interrupt);
    return () => {
      invalidate();
      stop();
      resize.disconnect();
      viewport.removeEventListener("scroll", scroll);
      for (const input of inputs)
        viewport.removeEventListener(input, interrupt);
      preference.removeEventListener("change", interrupt);
    };
  }, [capture, container, invalidate, stop]);
}
