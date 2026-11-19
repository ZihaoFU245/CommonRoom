import type { RefObject } from "preact";
import { useLayoutEffect, useMemo, useRef, useState } from "preact/hooks";
import type { TimelineEntry } from "../api/protocol.ts";
import { observedBorderBox } from "../features/conversation/observedBorderBox.ts";
import { TimelineScrollMotion } from "../features/conversation/timelineScrollMotion.ts";
import {
  indexAt,
  timelineOffsets,
  virtualWindow,
  anchoredScrollTop,
} from "../features/conversation/virtualTimeline.ts";
export interface VirtualTimelineHandle {
  anchor: () => number | undefined;
  position: () => { sequence: number; offset: number } | undefined;
  offsetForSequence?: (sequence: number) => number | undefined;
  scrollToSequence: (
    sequence: number,
    offset?: number,
    smooth?: boolean,
    center?: boolean,
    complete?: () => void,
  ) => void;
  isScrolling?: () => boolean;
  cancelScroll?: () => void;
  scrollToBottom?: (smooth?: boolean) => void;
}
export function useVirtualTimeline(
  entries: TimelineEntry[],
  container: RefObject<HTMLElement>,
  handle: RefObject<VirtualTimelineHandle>,
  atBottom: RefObject<boolean>,
  renderVersion: unknown = 0,
  scope = "",
) {
  const sizes = useRef(new Map<string, number>());
  const rowMeasurements = useRef({
    rects: new Map<HTMLElement, DOMRect>(),
    scrollTop: 0,
  });
  useLayoutEffect(() => {
    rowMeasurements.current.rects.clear();
  });
  const observed = useRef(new Set<HTMLElement>());
  const observer = useRef<ResizeObserver | null>(null);
  const [measurements, setMeasurements] = useState(0);
  const viewportTop = useRef(0);
  const currentOffsets = useRef<number[]>([]);
  const currentEntries = useRef(entries);
  currentEntries.current = entries;
  const scrollMotion = useRef<TimelineScrollMotion | null>(null);
  const touchScrollScope = useRef<string>();
  const currentScope = useRef(scope);
  currentScope.current = scope;
  const layoutShift = useRef(0);
  const renderedWindow = useRef<{ start: number; end: number } | null>(null);
  const [viewport, setViewport] = useState({ start: 0, end: 0, height: 600 });
  // Async children can remount during scrolling. Retain this row's existing
  // measurement until their loading placeholders resolve; scrolling/anchoring
  // stays owned here instead of each media component keeping its own cache.
  const restorationFrames = useRef(new Map<HTMLElement, number>());
  function preservePendingHeight(element: HTMLElement) {
    const key = element.dataset.virtualKey;
    const height = key ? sizes.current.get(key) : undefined;
    const pending =
      element.querySelector("[data-layout-pending]") !== null ||
      [...element.querySelectorAll("img")].some((image) => !image.complete);
    if (height && pending) {
      const frame = restorationFrames.current.get(element);
      if (frame !== undefined) cancelAnimationFrame(frame);
      restorationFrames.current.delete(element);
      element.style.minHeight = `${height}px`;
      element.dataset.layoutRestoring = "";
    } else if (element.dataset.layoutRestoring !== undefined) {
      if (restorationFrames.current.has(element)) return;
      // Let nested ResizeObservers see the restoration marker before releasing
      // the reservation. They must not replay the original loading animation.
      restorationFrames.current.set(
        element,
        requestAnimationFrame(() => {
          restorationFrames.current.set(
            element,
            requestAnimationFrame(() => {
              restorationFrames.current.delete(element);
              element.style.minHeight = "";
              delete element.dataset.layoutRestoring;
            }),
          );
        }),
      );
    }
  }
  function updateViewport(top: number, height: number) {
    viewportTop.current = top;
    const next = virtualWindow(
      currentOffsets.current,
      top + layoutShift.current,
      height,
    );
    setViewport((previous) =>
      previous.start === next.start &&
      previous.end === next.end &&
      previous.height === height &&
      renderedWindow.current?.start === next.start &&
      renderedWindow.current.end === next.end
        ? previous
        : { start: next.start, end: next.end, height },
    );
  }
  const keys = useMemo(() => entries.map((entry) => entry.key), [entries]);
  const { offsets, layoutVersion } = useMemo(
    () => ({
      offsets: timelineOffsets(keys, sizes.current),
      layoutVersion: measurements,
    }),
    [keys, measurements],
  );
  currentOffsets.current = offsets;
  const previous = useRef<{
    keys: string[];
    offsets: number[];
    scope: string;
  }>();
  // Replacing a history window can shrink the scroll extent during DOM commit.
  // Keep the old coordinate before the browser clamps scrollTop to that extent.
  const beforeCommitTop = container.current?.scrollTop ?? viewportTop.current;
  const old = previous.current;
  const oldShift = old?.scope === scope ? layoutShift.current : 0;
  const oldTop = beforeCommitTop + oldShift;
  const touchScrolling = touchScrollScope.current === scope;
  const anchorTop =
    old?.scope === scope &&
    old.offsets !== offsets &&
    (touchScrolling || oldTop > 0)
      ? anchoredScrollTop(old.keys, old.offsets, keys, offsets, oldTop)
      : oldTop;
  // Move the spacers by the anchor correction during a touch scroll. The rows
  // stay in place without a scrollTop write that would stop native momentum.
  const shift = touchScrolling ? anchorTop - beforeCommitTop : 0;
  useLayoutEffect(() => {
    handle.current = {
      anchor: () => {
        const index = indexAt(
          offsets,
          (container.current?.scrollTop ?? 0) + layoutShift.current,
        );
        const entry = entries
          .slice(index)
          .find((entry) => entry.kind === "message");
        return entry?.kind === "message" ? entry.message.sequence : undefined;
      },
      position: () => {
        const top = (container.current?.scrollTop ?? 0) + layoutShift.current;
        const first = indexAt(offsets, top);
        const index = entries.findIndex(
          (entry, index) => index >= first && entry.kind === "message",
        );
        const entry = entries[index];
        return entry?.kind === "message"
          ? {
              sequence: entry.message.sequence,
              offset: (offsets[index] ?? 0) - top,
            }
          : undefined;
      },
      offsetForSequence: (sequence) => {
        const index = entries.findIndex(
          (entry) =>
            entry.kind === "message" && entry.message.sequence === sequence,
        );
        return index < 0
          ? undefined
          : (offsets[index] ?? 0) -
              (container.current?.scrollTop ?? 0) -
              layoutShift.current;
      },
      isScrolling: () => scrollMotion.current?.active ?? false,
      cancelScroll: () => scrollMotion.current?.cancel(),
      scrollToBottom: (smooth = false) => {
        const element = container.current;
        if (!element) return;
        const target = () =>
          Math.max(0, element.scrollHeight - element.clientHeight);
        const complete = () => {
          atBottom.current = true;
          element.scrollTop = target();
          updateViewport(element.scrollTop, element.clientHeight);
        };
        if (smooth && scrollMotion.current) {
          atBottom.current = false;
          scrollMotion.current.scrollTo(target, complete);
        } else {
          scrollMotion.current?.cancel();
          complete();
        }
      },
      scrollToSequence: (
        sequence,
        offset = 0,
        smooth = false,
        center = false,
        complete,
      ) => {
        const index = entries.findIndex(
          (entry) =>
            entry.kind === "message" && entry.message.sequence === sequence,
        );
        const element = container.current;
        if (index >= 0 && element) {
          scrollMotion.current?.cancel();
          if (scrollMotion.current) {
            atBottom.current = false;
            const target = () => {
              const currentIndex = currentEntries.current.findIndex(
                (entry) =>
                  entry.kind === "message" &&
                  entry.message.sequence === sequence,
              );
              return currentIndex < 0
                ? undefined
                : Math.max(
                    0,
                    (currentOffsets.current[currentIndex] ?? 0) -
                      layoutShift.current -
                      (center
                        ? (element.clientHeight -
                            ((currentOffsets.current[currentIndex + 1] ?? 0) -
                              (currentOffsets.current[currentIndex] ?? 0))) /
                          2
                        : offset),
                  );
            };
            if (smooth) scrollMotion.current.scrollTo(target, complete);
            else {
              scrollMotion.current.snapTo(target);
              complete?.();
            }
          } else {
            atBottom.current = false;
            element.scrollTop = Math.max(
              0,
              (offsets[index] ?? 0) -
                layoutShift.current -
                (center
                  ? (element.clientHeight -
                      ((offsets[index + 1] ?? 0) - (offsets[index] ?? 0))) /
                    2
                  : offset),
            );
            updateViewport(element.scrollTop, element.clientHeight);
            complete?.();
          }
        }
      },
    };
  });
  useLayoutEffect(() => {
    const element = container.current;
    if (!element) return;
    layoutShift.current = shift;
    const currentTop = element.scrollTop;
    if (
      !touchScrolling &&
      !scrollMotion.current?.active &&
      (atBottom.current || Math.abs(anchorTop - currentTop) > 0.5) &&
      // Visible folds and bottom-pinned resize animations own their correction.
      (oldShift ||
        !element.querySelector(
          "[data-message-resizing], [data-content-resizing]",
        ))
    ) {
      const max = Math.max(0, element.scrollHeight - element.clientHeight);
      const target = atBottom.current
        ? max
        : Math.min(max, Math.max(0, anchorTop));
      if (Math.abs(target - currentTop) > 0.5) element.scrollTop = target;
    }
    previous.current = { keys, offsets, scope };
    updateViewport(element.scrollTop, element.clientHeight);
  }, [
    keys,
    offsets,
    viewport.height,
    atBottom,
    anchorTop,
    container,
    oldShift,
    scope,
    shift,
    touchScrolling,
  ]);
  useLayoutEffect(() => {
    const live = new Set(keys);
    for (const key of sizes.current.keys())
      if (!live.has(key)) sizes.current.delete(key);
  }, [keys]);
  useLayoutEffect(() => {
    const element = container.current;
    if (!element) return;
    const pendingRestorations = restorationFrames.current;
    const motion = new TimelineScrollMotion(element, () =>
      updateViewport(element.scrollTop, element.clientHeight),
    );
    scrollMotion.current = motion;
    const interrupt = () => motion.cancel();
    const inputs = ["wheel", "pointerdown", "keydown"] as const;
    for (const input of inputs)
      element.addEventListener(input, interrupt, { passive: true });
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    const reduceMotion = () => {
      if (preference.matches && motion.active) motion.finish();
    };
    preference.addEventListener("change", reduceMotion);
    let frame = 0;
    const activeTouches = new Set<number>();
    const touchTargets = new Set<EventTarget>();
    let settleTimer: number | undefined;
    const endTouchScroll = () => {
      if (activeTouches.size || touchScrollScope.current === undefined) return;
      window.clearTimeout(settleTimer);
      touchScrollScope.current = undefined;
      // Re-render to restore the spacers before writing the final correction.
      if (layoutShift.current || atBottom.current)
        setViewport((previous) => ({ ...previous }));
    };
    const scheduleTouchEnd = () => {
      window.clearTimeout(settleTimer);
      settleTimer = window.setTimeout(endTouchScroll, 150);
    };
    const touchStart = (event: TouchEvent) => {
      const target = event.target;
      if (!target) return;
      interrupt();
      window.clearTimeout(settleTimer);
      touchScrollScope.current = currentScope.current;
      for (const touch of Array.from(event.changedTouches))
        activeTouches.add(touch.identifier);
      // A virtual row can unmount mid-gesture. Release still targets that node,
      // so the scroller and document may never see it bubble.
      if (!touchTargets.has(target)) {
        touchTargets.add(target);
        target.addEventListener("touchend", touchEnd, { passive: true });
        target.addEventListener("touchcancel", touchEnd, { passive: true });
      }
    };
    const clearTouchTargets = () => {
      for (const target of touchTargets) {
        target.removeEventListener("touchend", touchEnd);
        target.removeEventListener("touchcancel", touchEnd);
      }
      touchTargets.clear();
    };
    const touchEnd: EventListener = (event) => {
      if (!activeTouches.size) return;
      for (const touch of Array.from((event as TouchEvent).changedTouches))
        activeTouches.delete(touch.identifier);
      if (activeTouches.size) return;
      clearTouchTargets();
      scheduleTouchEnd();
    };
    const refresh = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() =>
        updateViewport(element.scrollTop, element.clientHeight),
      );
    };
    const scroll = () => {
      if (touchScrollScope.current !== undefined && !activeTouches.size)
        scheduleTouchEnd();
      refresh();
    };
    const updatePendingRows = () => {
      for (const row of observed.current) preservePendingHeight(row);
    };
    const contentChanges = new MutationObserver(updatePendingRows);
    element.addEventListener("load", updatePendingRows, true);
    element.addEventListener("error", updatePendingRows, true);
    contentChanges.observe(element, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-layout-pending"],
    });
    const resize = new ResizeObserver(refresh);
    resize.observe(element);
    observer.current = new ResizeObserver((records) => {
      let changed = false;
      for (const record of records) {
        const key =
          record.target instanceof HTMLElement
            ? record.target.dataset.virtualKey
            : undefined;
        const height = observedBorderBox(record).height;
        if (
          key &&
          height > 0 &&
          Math.abs((sizes.current.get(key) ?? 100) - height) > 0.5
        ) {
          sizes.current.set(key, height);
          changed = true;
        }
      }
      if (changed) setMeasurements((value) => value + 1);
    });
    element.addEventListener("scroll", scroll, { passive: true });
    element.addEventListener("scrollend", endTouchScroll);
    element.addEventListener("touchstart", touchStart, { passive: true });
    const owner = element.ownerDocument;
    // Also see releases that the browser retargets to the document.
    owner.addEventListener("touchend", touchEnd, {
      capture: true,
      passive: true,
    });
    owner.addEventListener("touchcancel", touchEnd, {
      capture: true,
      passive: true,
    });
    refresh();
    return () => {
      motion.cancel();
      scrollMotion.current = null;
      window.clearTimeout(settleTimer);
      touchScrollScope.current = undefined;
      for (const input of inputs) element.removeEventListener(input, interrupt);
      preference.removeEventListener("change", reduceMotion);
      cancelAnimationFrame(frame);
      resize.disconnect();
      contentChanges.disconnect();
      element.removeEventListener("load", updatePendingRows, true);
      element.removeEventListener("error", updatePendingRows, true);
      observer.current?.disconnect();
      observer.current = null;
      observed.current.clear();
      for (const frame of pendingRestorations.values())
        cancelAnimationFrame(frame);
      pendingRestorations.clear();
      element.removeEventListener("scroll", scroll);
      element.removeEventListener("scrollend", endTouchScroll);
      element.removeEventListener("touchstart", touchStart);
      owner.removeEventListener("touchend", touchEnd, true);
      owner.removeEventListener("touchcancel", touchEnd, true);
      clearTouchTargets();
    };
  }, [atBottom, container]);
  const visibleWindow = virtualWindow(
    offsets,
    oldShift || shift ? anchorTop : viewportTop.current,
    viewport.height,
  );
  // Anchoring can restore the previous window after this render computed a
  // different one from new sizes/entries. Compare against what was just mounted.
  renderedWindow.current = visibleWindow;
  useLayoutEffect(() => {
    const currentObserver = observer.current;
    if (!currentObserver) return;
    const liveElements = new Set(
      container.current?.querySelectorAll<HTMLElement>(
        ".timeline-rows > [data-virtual-key]",
      ) ?? [],
    );
    for (const element of observed.current) {
      if (!liveElements.has(element)) {
        currentObserver.unobserve(element);
        const frame = restorationFrames.current.get(element);
        if (frame !== undefined) cancelAnimationFrame(frame);
        restorationFrames.current.delete(element);
      }
    }
    let changed = false;
    rowMeasurements.current.scrollTop = container.current?.scrollTop ?? 0;
    for (const element of liveElements) {
      const key = element.dataset.virtualKey;
      preservePendingHeight(element);
      const rect = element.getBoundingClientRect();
      rowMeasurements.current.rects.set(element, rect);
      const height = rect.height;
      if (
        key &&
        height > 0 &&
        Math.abs((sizes.current.get(key) ?? 100) - height) > 0.5
      ) {
        sizes.current.set(key, height);
        changed = true;
      }
      if (!observed.current.has(element))
        currentObserver.observe(element, { box: "border-box" });
    }
    observed.current = liveElements;
    // Resolve new/edited row sizes before motion captures the final positions.
    if (changed) setMeasurements((value) => value + 1);
  }, [
    entries,
    renderVersion,
    visibleWindow.start,
    visibleWindow.end,
    container,
  ]);
  return {
    rowMeasurements: rowMeasurements.current,
    visible: entries.slice(visibleWindow.start, visibleWindow.end),
    start: visibleWindow.start,
    end: visibleWindow.end,
    layoutVersion,
    top: visibleWindow.top - shift,
    bottom: visibleWindow.bottom,
  };
}
