import type { VirtualTimelineHandle } from "./useVirtualTimeline.ts";
import type { RefObject } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import type { Message, Unread, LocalOutput } from "../api/protocol.ts";
import { useEvent } from "./useEvent.ts";
import { api } from "../api/client.ts";
import { visibleReadPosition } from "../features/conversation/unread.ts";
import {
  bottomShortcut,
  UnreadJumpTargets,
  retainedJumpSequence,
} from "../features/conversation/historyNavigation.ts";
interface Context {
  selected: string;
  conversationId?: string | undefined;
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
  hasAfter?: boolean;
  loadLatest?: () => void;
  historyLoading?: boolean;
}
export function useReadTracking({
  selected,
  conversationId,
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
  hasAfter = false,
  loadLatest,
  historyLoading = false,
}: Context) {
  const [unreadBoundary, setUnreadBoundary] = useState<number | null>(null);
  const enteredView = useRef<string | null>(null);
  const positions = useRef(
    new Map<string, { bottom: boolean; sequence?: number; offset: number }>(),
  );
  const identity = conversationId ?? selected;
  const pendingJump = useRef<{
    identity: string;
    sequence: number;
    previous: Message[];
  } | null>(null);
  const pendingBottom = useRef<string | null>(null);
  const unreadJumps = useRef(new UnreadJumpTargets());
  const [navigation, setNavigation] = useState({
    identity,
    bottom: false,
    unread: false,
  });
  const updateNavigation = useEvent(() => {
    const viewport = historyElement.current;
    if (!viewport || !historyReady) return;
    // Keep Safari's elastic positions within the actual scrollable range.
    const maximum = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
    const top = Math.min(maximum, Math.max(0, viewport.scrollTop));
    const distance = maximum - top;
    const away = hasAfter || !atBottom.current;
    const sequence = virtualTimeline.current?.position()?.sequence;
    const unreadTarget = unreadJumps.current.target(
      identity,
      unread?.first,
      unread?.count ?? 0,
    );
    setNavigation((previous) => {
      const bottom =
        away &&
        bottomShortcut(
          distance,
          viewport.clientHeight,
          hasAfter,
          previous.identity === identity && previous.bottom,
        );
      const unreadAhead =
        away &&
        unreadTarget !== undefined &&
        (sequence === undefined || sequence <= unreadTarget);
      return previous.identity === identity &&
        previous.bottom === bottom &&
        previous.unread === unreadAhead
        ? previous
        : { identity, bottom, unread: unreadAhead };
    });
  });
  useLayoutEffect(() => {
    virtualTimeline.current?.cancelScroll?.();
    pendingJump.current = null;
    pendingBottom.current = null;
  }, [identity]);
  const restoreAttempt = useRef<string | null>(null);
  const rememberPosition = useEvent(() => {
    if (enteredView.current !== identity) return;
    const position = virtualTimeline.current?.position();
    const sequence = position?.sequence;
    positions.current.delete(identity);
    positions.current.set(identity, {
      bottom: atBottom.current === true,
      ...(sequence === undefined ? {} : { sequence }),
      offset: position?.offset ?? 0,
    });
    if (positions.current.size > 32) {
      const oldest = positions.current.keys().next().value;
      if (oldest !== undefined) positions.current.delete(oldest);
    }
  });
  const readSent = useRef(new Map<string, number>());
  const readBusy = useRef(false);
  const readQueued = useRef(false);
  const mounted = useRef(true);
  const readTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  function scrollToBottom() {
    const viewport = historyElement.current;
    if (viewport)
      viewport.scrollTop = viewport.scrollHeight - viewport.clientHeight;
  }
  useLayoutEffect(() => {
    const jump = pendingJump.current;
    if (
      jump?.identity === identity &&
      historyReady &&
      !historyLoading &&
      messages !== jump.previous
    ) {
      pendingJump.current = null;
      atBottom.current = false;
      const nearest = retainedJumpSequence(
        messages.map((message) => message.sequence),
        jump.sequence,
      );
      if (nearest !== undefined) {
        setUnreadBoundary(nearest);
        resetCleared();
        virtualTimeline.current?.scrollToSequence(nearest, 0, true);
      }
    }
    if (pendingBottom.current === identity && historyReady && !hasAfter) {
      pendingBottom.current = null;
      virtualTimeline.current?.scrollToBottom?.(true);
    }
    if (enteredView.current !== identity && (!unread || historyReady)) {
      const saved = positions.current.get(identity);
      if (
        saved?.sequence !== undefined &&
        unread &&
        (saved.sequence < unread.oldest || saved.sequence > unread.through)
      )
        positions.current.delete(identity);
      const position = positions.current.get(identity);
      if (position && !position.bottom && position.sequence !== undefined) {
        let sequence = position.sequence;
        if (!messages.some((message) => message.sequence === sequence)) {
          const attempt = `${identity}:${sequence}`;
          const covers =
            messages.length > 0 &&
            (messages[0]?.sequence ?? Infinity) <= sequence &&
            (messages.at(-1)?.sequence ?? -Infinity) >= sequence;
          if (!covers && restoreAttempt.current !== attempt) {
            restoreAttempt.current = attempt;
            loadSequence(sequence);
            return;
          }
          // A retracted anchor must not cause an endless around-page loop.
          const nearest =
            messages.find((message) => message.sequence >= sequence) ??
            messages.at(-1);
          if (nearest) sequence = nearest.sequence;
          else positions.current.delete(identity);
        }
        if (messages.length) {
          restoreAttempt.current = null;
          enteredView.current = identity;
          atBottom.current = false;
          virtualTimeline.current?.scrollToSequence(sequence, position.offset);
          setUnreadBoundary(unread?.first ?? null);
          updateNavigation();
          return;
        }
      }
      restoreAttempt.current = null;
      enteredView.current = identity;
      const first = position?.bottom ? undefined : unread?.first;
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
        scrollToBottom();
        atBottom.current = true;
      }
    } else if (enteredView.current === identity && atBottom.current) {
      scrollToBottom();
    }
    rememberPosition();
    updateNavigation();
  }, [
    messages,
    selected,
    identity,
    historyReady,
    output,
    cleared,
    unread,
    hasAfter,
    historyLoading,
  ]);
  function acknowledgeVisible() {
    if (readTimer.current !== undefined) return;
    readTimer.current = setTimeout(() => {
      readTimer.current = undefined;
      recordVisibleRead().catch((error: unknown) => {
        console.error("Read tracking failed", error);
      });
    }, 250);
  }
  const recordVisibleRead = useEvent(async () => {
    if (
      !historyReady ||
      document.hidden ||
      !document.hasFocus() ||
      settingsOpen ||
      (menu && window.matchMedia("(max-width: 700px)").matches)
    )
      return;
    if (readBusy.current) {
      readQueued.current = true;
      return;
    }
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
    if (!through || (readSent.current.get(identity) || 0) >= through) return;
    const view = selected;
    const readIdentity = identity;
    readBusy.current = true;
    try {
      await api.read(view, through);
      readSent.current.delete(readIdentity);
      readSent.current.set(readIdentity, through);
      if (readSent.current.size > 32) {
        const oldest = readSent.current.keys().next().value;
        if (oldest !== undefined) readSent.current.delete(oldest);
      }
    } catch {
      /* Retry on the next snapshot, focus, or scroll. */
    } finally {
      readBusy.current = false;
      if (readQueued.current && mounted.current) {
        readQueued.current = false;
        acknowledgeVisible();
      }
    }
  });
  useEffect(() => {
    acknowledgeVisible();
    window.addEventListener("focus", acknowledgeVisible);
    document.addEventListener("visibilitychange", acknowledgeVisible);
    return () => {
      window.removeEventListener("focus", acknowledgeVisible);
      document.removeEventListener("visibilitychange", acknowledgeVisible);
    };
  }, [selected, messages, unread, historyReady, menu, settingsOpen, status]);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      readQueued.current = false;
      clearTimeout(readTimer.current);
    };
  }, []);
  function jumpToUnread() {
    const target = unreadJumps.current.consume(identity);
    if (target === undefined) return;
    const first = Math.max(target, unread?.oldest ?? target);
    pendingBottom.current = null;
    atBottom.current = false;
    updateNavigation();
    const covers =
      (messages[0]?.sequence ?? Infinity) <= first &&
      (messages.at(-1)?.sequence ?? -Infinity) >= first;
    if (!historyReady || !covers) {
      pendingJump.current = { identity, sequence: first, previous: messages };
      loadSequence(first);
      return;
    }
    const nearest = retainedJumpSequence(
      messages.map((message) => message.sequence),
      first,
    );
    if (nearest === undefined) return;
    resetCleared();
    setUnreadBoundary(nearest);
    atBottom.current = false;
    virtualTimeline.current?.scrollToSequence(nearest, 0, true);
    requestAnimationFrame(acknowledgeVisible);
  }

  return {
    unreadBoundary,
    acknowledgeVisible,
    rememberPosition,
    jumpToUnread,
    updateNavigation,
    showJumpToBottom: navigation.identity === identity && navigation.bottom,
    showJumpToUnread: navigation.identity === identity && navigation.unread,
    jumpToBottom: () => {
      pendingJump.current = null;
      if (hasAfter || !historyReady) {
        pendingBottom.current = identity;
        loadLatest?.();
      } else {
        virtualTimeline.current?.scrollToBottom?.(true);
      }
    },
    resetEntry: () => {
      enteredView.current = null;
      positions.current.delete(identity);
    },
  };
}
