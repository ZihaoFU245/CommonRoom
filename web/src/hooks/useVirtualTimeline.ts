import type { RefObject } from "preact";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "preact/hooks";
import type { TimelineEntry } from "../api/protocol.ts";
import {
  indexAt,
  timelineOffsets,
  virtualWindow,
  anchoredScrollTop,
} from "../features/conversation/virtualTimeline.ts";
export interface VirtualTimelineHandle {
  anchor: () => number | undefined;
  scrollToSequence: (sequence: number) => void;
}
export function useVirtualTimeline(
  entries: TimelineEntry[],
  container: RefObject<HTMLElement>,
  handle: RefObject<VirtualTimelineHandle>,
  atBottom: RefObject<boolean>,
  renderVersion = 0,
) {
  const sizes = useRef(new Map<string, number>());
  const observer = useRef<ResizeObserver | null>(null);
  const measuredWindow = useRef<{
    entries: TimelineEntry[];
    start: number;
    end: number;
    renderVersion: number;
  } | null>(null);
  const [measurements, setMeasurements] = useState(0);
  const [viewport, setViewport] = useState({ top: 0, height: 600 });
  const offsets = useMemo(() => {
    return timelineOffsets(
      entries.map((entry) => entry.key),
      sizes.current,
    );
  }, [entries, measurements]);
  const previous = useRef<{ keys: string[]; offsets: number[] } | null>(null);
  useLayoutEffect(() => {
    handle.current = {
      anchor: () => {
        const index = indexAt(offsets, container.current?.scrollTop ?? 0);
        const entry = entries
          .slice(index)
          .find((entry) => entry.kind === "message");
        return entry?.kind === "message" ? entry.message.sequence : undefined;
      },
      scrollToSequence: (sequence) => {
        const index = entries.findIndex(
          (entry) =>
            entry.kind === "message" && entry.message.sequence === sequence,
        );
        const element = container.current;
        if (index >= 0 && element) {
          element.scrollTop = offsets[index] ?? 0;
          setViewport({ top: element.scrollTop, height: element.clientHeight });
        }
      },
    };
  });
  useLayoutEffect(() => {
    const element = container.current;
    if (!element) return;
    const old = previous.current;
    if (atBottom.current) {
      element.scrollTop = element.scrollHeight - element.clientHeight;
    } else if (old && element.scrollTop > 0) {
      element.scrollTop = anchoredScrollTop(
        old.keys,
        old.offsets,
        entries.map((entry) => entry.key),
        offsets,
        element.scrollTop,
      );
    }
    previous.current = { keys: entries.map((entry) => entry.key), offsets };
    setViewport({ top: element.scrollTop, height: element.clientHeight });
    const live = new Set(entries.map((entry) => entry.key));
    for (const key of sizes.current.keys())
      if (!live.has(key)) sizes.current.delete(key);
  }, [entries, offsets, viewport.height]);
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    let frame = 0;
    const refresh = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() =>
        setViewport({ top: element.scrollTop, height: element.clientHeight }),
      );
    };
    const resize = new ResizeObserver(refresh);
    resize.observe(element);
    observer.current = new ResizeObserver((records) => {
      let changed = false;
      for (const record of records) {
        const key =
          record.target instanceof HTMLElement
            ? record.target.dataset.virtualKey
            : undefined;
        const height = record.target.getBoundingClientRect().height;
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
    element.addEventListener("scroll", refresh, { passive: true });
    refresh();
    return () => {
      cancelAnimationFrame(frame);
      resize.disconnect();
      observer.current?.disconnect();
      observer.current = null;
      element.removeEventListener("scroll", refresh);
    };
  }, []);
  const window = virtualWindow(offsets, viewport.top, viewport.height);
  useLayoutEffect(() => {
    const currentObserver = observer.current;
    currentObserver?.disconnect();
    const measured = measuredWindow.current;
    const measure =
      measured?.entries !== entries ||
      measured.renderVersion !== renderVersion ||
      measured.start !== window.start ||
      measured.end !== window.end;
    measuredWindow.current = {
      entries,
      start: window.start,
      end: window.end,
      renderVersion,
    };
    let changed = false;
    for (const element of container.current?.querySelectorAll<HTMLElement>(
      ".timeline-rows > [data-virtual-key]",
    ) ?? []) {
      if (measure) {
        const key = element.dataset.virtualKey;
        const height = element.getBoundingClientRect().height;
        if (
          key &&
          height > 0 &&
          Math.abs((sizes.current.get(key) ?? 100) - height) > 0.5
        ) {
          sizes.current.set(key, height);
          changed = true;
        }
      }
      currentObserver?.observe(element);
    }
    // Resolve new/edited row sizes before motion captures the final positions.
    if (changed) setMeasurements((value) => value + 1);
  });
  return {
    visible: entries.slice(window.start, window.end),
    start: window.start,
    end: window.end,
    layoutVersion: measurements,
    top: window.top,
    bottom: window.bottom,
  };
}
