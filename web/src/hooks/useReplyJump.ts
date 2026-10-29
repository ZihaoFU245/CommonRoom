import type { RefObject } from "preact";
import { useLayoutEffect, useRef } from "preact/hooks";
import { useEvent } from "./useEvent.ts";
import type { VirtualTimelineHandle } from "./useVirtualTimeline.ts";
import type { Reply, TimelineEntry } from "../api/protocol.ts";

export function useReplyJump(
  entries: TimelineEntry[],
  motionScope: string,
  historyElement: RefObject<HTMLElement>,
  virtualTimeline: RefObject<VirtualTimelineHandle>,
  atBottom: RefObject<boolean>,
  loadSequence: (sequence: number) => void,
  virtualWindow: { start: number; end: number; layoutVersion: number },
) {
  // A reply can point outside the virtual window, so scroll the timeline to its
  // sequence and center the row once that window renders.
  const pendingReply = useRef<string | null>(null);
  const highlight = useRef<Animation | null>(null);
  useLayoutEffect(() => {
    const reduced = globalThis.matchMedia("(prefers-reduced-motion: reduce)");
    const stopHighlight = () => {
      highlight.current?.cancel();
      highlight.current = null;
    };
    reduced.addEventListener("change", stopHighlight);
    return () => {
      pendingReply.current = null;
      stopHighlight();
      reduced.removeEventListener("change", stopHighlight);
    };
  }, [motionScope]);
  const reveal = useEvent((id: string, scroll = true) => {
    const element = document.getElementById(id);
    if (!element || !historyElement.current?.contains(element)) return false;
    atBottom.current = false;
    if (scroll) element.scrollIntoView({ block: "center" });
    // Reuse the hover wash so a jump is easy to follow, then let it fade.
    highlight.current?.cancel();
    highlight.current = null;
    if (globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches)
      return true;
    highlight.current = element.animate(
      [
        { backgroundColor: "var(--wash)" },
        { backgroundColor: "var(--wash)", offset: 0.6 },
        { backgroundColor: "transparent" },
      ],
      { duration: 1200, easing: "ease-out" },
    );
    return true;
  });
  const jumpToReply = useEvent((reply: Reply) => {
    // A second click supersedes a jump whose history request is still pending.
    pendingReply.current = null;
    const id = `message-${reply.id}`;
    const entry = entries.find(
      (entry) => entry.kind === "message" && entry.message.id === reply.id,
    );
    if (entry?.kind === "message") {
      atBottom.current = false;
      virtualTimeline.current?.scrollToSequence(
        entry.message.sequence,
        0,
        true,
        true,
        () => {
          reveal(id, false);
        },
      );
      return;
    }
    virtualTimeline.current?.cancelScroll?.();
    // The original is outside the loaded history; page around it by sequence.
    if (reply.sequence === undefined || reply.sequence === 0) return;
    atBottom.current = false;
    pendingReply.current = id;
    loadSequence(reply.sequence);
  });
  useLayoutEffect(() => {
    const id = pendingReply.current;
    if (id === null) return;
    if (reveal(id)) {
      pendingReply.current = null;
      return;
    }
    const entry = entries.find(
      (entry) =>
        entry.kind === "message" && `message-${entry.message.id}` === id,
    );
    if (entry?.kind !== "message") return;
    atBottom.current = false;
    virtualTimeline.current?.scrollToSequence(entry.message.sequence);
  }, [
    entries,
    atBottom,
    reveal,
    virtualTimeline,
    virtualWindow.start,
    virtualWindow.end,
    virtualWindow.layoutVersion,
  ]);
  return jumpToReply;
}
