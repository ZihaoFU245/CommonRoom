import type { VirtualTimelineHandle } from "./useVirtualTimeline.ts";
import type { RefObject } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import type { Message, Unread, LocalOutput } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { visibleReadPosition } from "../features/conversation/unread.ts";
interface Context {
  selected: string;
  unread: Unread | undefined;
  messages: Message[];
  historyReady: boolean;
  output: LocalOutput[];
  cleared: Record<string, number>;
  resetCleared: () => void;
  settingsOpen: boolean;
  menu: boolean;
  status: string;
  loadSequence: (sequence: number) => void;
  virtualTimeline: RefObject<VirtualTimelineHandle>;
  atBottom: RefObject<boolean>;
  historyElement: RefObject<HTMLElement>;
  end: RefObject<HTMLDivElement>;
}
export function useReadTracking({
  selected,
  unread,
  messages,
  historyReady,
  output,
  cleared,
  resetCleared,
  settingsOpen,
  menu,
  status,
  loadSequence,
  virtualTimeline,
  atBottom,
  historyElement,
  end,
}: Context) {
  const [unreadBoundary, setUnreadBoundary] = useState<number | null>(null);
  const enteredView = useRef<string | null>(null);
  const readSent = useRef(new Map<string, number>());
  const readBusy = useRef(false);
  const readTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  useLayoutEffect(() => {
    if (enteredView.current !== selected && (!unread || historyReady)) {
      enteredView.current = selected;
      const first = unread?.first;
      if (first && !messages.some((message) => message.sequence === first)) {
        loadSequence(first);
        enteredView.current = null;
        return;
      }
      setUnreadBoundary(first || null);
      if (first) {
        resetCleared();
        virtualTimeline.current?.scrollToSequence(first);
        atBottom.current = false;
      } else {
        end.current?.scrollIntoView({ block: "end" });
        atBottom.current = true;
      }
    } else if (enteredView.current === selected && atBottom.current) {
      end.current?.scrollIntoView({ block: "end" });
    }
  }, [messages, selected, historyReady, output, cleared]);
  function acknowledgeVisible() {
    clearTimeout(readTimer.current);
    readTimer.current = setTimeout(() => {
      recordVisibleRead().catch((error: unknown) => {
        console.error("Read tracking failed", error);
      });
    }, 250);
  }
  async function recordVisibleRead() {
    if (
      readBusy.current ||
      !historyReady ||
      document.hidden ||
      !document.hasFocus() ||
      settingsOpen ||
      (menu && window.matchMedia("(max-width: 700px)").matches)
    )
      return;
    const viewport = historyElement.current;
    if (!viewport) return;
    const elements = [
      ...viewport.querySelectorAll<HTMLElement>("[data-sequence]"),
    ].map((element) => ({
      sequence: Number(element.dataset.sequence),
      top: element.getBoundingClientRect().top,
      bottom: element.getBoundingClientRect().bottom,
    }));
    const through = visibleReadPosition(
      elements,
      viewport.getBoundingClientRect(),
      messages.some((message) => message.sequence === unread?.first)
        ? null
        : (unread?.first ?? null),
    );
    if (!through || (readSent.current.get(selected) || 0) >= through) return;
    const view = selected;
    readBusy.current = true;
    try {
      await api.read(view, through);
      readSent.current.set(view, through);
    } catch {
      /* Retry on the next snapshot, focus, or scroll. */
    } finally {
      readBusy.current = false;
    }
  }
  useEffect(() => {
    acknowledgeVisible();
    window.addEventListener("focus", acknowledgeVisible);
    document.addEventListener("visibilitychange", acknowledgeVisible);
    return () => {
      clearTimeout(readTimer.current);
      window.removeEventListener("focus", acknowledgeVisible);
      document.removeEventListener("visibilitychange", acknowledgeVisible);
    };
  }, [selected, messages, unread, historyReady, menu, settingsOpen, status]);
  function jumpToUnread() {
    if (!unread?.first) return;
    if (
      !historyReady ||
      !messages.some((message) => message.sequence === unread.first)
    ) {
      loadSequence(unread.first);
      return;
    }
    resetCleared();
    setUnreadBoundary(unread?.first ?? null);
    atBottom.current = false;
    virtualTimeline.current?.scrollToSequence(unread.first);
    requestAnimationFrame(acknowledgeVisible);
  }

  return {
    unreadBoundary,
    acknowledgeVisible,
    jumpToUnread,
    resetEntry: () => {
      enteredView.current = null;
    },
  };
}
