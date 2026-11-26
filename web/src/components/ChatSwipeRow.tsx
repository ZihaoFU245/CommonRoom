import type { JSX, VNode } from "preact";
import { useLayoutEffect, useRef } from "preact/hooks";
import { useEvent } from "../hooks/useEvent.ts";
import { canSwipeInPage } from "../features/preferences/inputFocus.ts";

const swipeWidth = 64;
const commitDistance = 48;
type SwipeAction = "read" | "unread";

export function ChatActionIcon({ kind }: { kind: "pin" | "read" | "unread" }) {
  return (
    <svg class="chat-action-icon" viewBox="0 0 24 24" aria-hidden="true">
      {kind === "pin" ? (
        <path d="m15 3 6 6-3 1-3 4v3l-1 1-8-8 1-1h3l4-3 1-3ZM10 14l-6 6" />
      ) : kind === "read" ? (
        <>
          <path class="chat-action-shape" d="M15 6H3l2 4v9h15v-8Z" />
          <circle class="chat-action-shape" cx="20" cy="6" r="2.5" />
          <path class="chat-read-lines" d="M8.5 11h8M8.5 15h5" />
        </>
      ) : (
        <circle class="chat-action-shape" cx="12" cy="12" r="5" />
      )}
    </svg>
  );
}

/** The browser owns horizontal panning and snapping back to the chat button. */
export function ChatSwipeRow({
  canRead,
  enabled,
  chatOrder,
  onSwipe,
  children,
}: {
  canRead: boolean;
  enabled: boolean;
  chatOrder: string;
  onSwipe: (action: SwipeAction) => void;
  children: VNode;
}) {
  const track = useRef<HTMLDivElement>(null);
  const gesture = useRef<{
    x: number;
    y: number;
    horizontal: boolean;
    eligible: boolean;
  }>();
  const suppressClick = useRef(false);
  const settling = useRef<{
    action: SwipeAction | undefined;
    timer: number;
    centered: boolean;
  }>();
  const commit = useEvent((action: SwipeAction) => {
    if (enabled && action === (canRead ? "read" : "unread")) onSwipe(action);
  });

  useLayoutEffect(() => {
    const element = track.current;
    if (!element) return;
    // Moving a keyed row can reset its nested scroll offset without a scroll event.
    element.scrollLeft = swipeWidth;
    return () => {
      gesture.current = undefined;
      if (settling.current) clearTimeout(settling.current.timer);
      settling.current = undefined;
      element.removeAttribute("data-chat-swipe-ready");
      element.removeAttribute("data-chat-swipe-settling");
      element.removeAttribute("data-chat-swipe-dragging");
    };
  }, [enabled, chatOrder]);

  function readyAction(element: HTMLElement): SwipeAction | undefined {
    const offset = element.scrollLeft - swipeWidth;
    if (canRead && offset <= -commitDistance) return "read";
    if (!canRead && offset >= commitDistance) return "unread";
    return undefined;
  }

  function completeReturn(element: HTMLElement) {
    const pending = settling.current;
    if (
      !pending ||
      pending.centered ||
      gesture.current ||
      Math.abs(element.scrollLeft - swipeWidth) >= 1
    )
      return;
    pending.centered = true;
    // Give the centered row a paint before the unread badge and order change.
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        if (settling.current !== pending) return;
        pending.centered = false;
        if (gesture.current || Math.abs(element.scrollLeft - swipeWidth) >= 1)
          return;
        clearTimeout(pending.timer);
        settling.current = undefined;
        element.removeAttribute("data-chat-swipe-ready");
        element.removeAttribute("data-chat-swipe-settling");
        if (pending.action) commit(pending.action);
      }),
    );
  }

  function returnToCenter(element: HTMLElement, action?: SwipeAction) {
    const previous = settling.current;
    if (previous) clearTimeout(previous.timer);
    action = previous?.action ?? action;
    if (action) element.dataset.chatSwipeReady = action;
    else element.removeAttribute("data-chat-swipe-ready");
    const pending = {
      action,
      centered: false,
      // An interrupted native animation can miss its final scroll event.
      timer: window.setTimeout(() => {
        if (settling.current !== pending || gesture.current) return;
        element.scrollTo({ left: swipeWidth, behavior: "instant" });
        completeReturn(element);
      }, 500),
    };
    settling.current = pending;
    element.setAttribute("data-chat-swipe-settling", "");
    requestAnimationFrame(() => {
      if (settling.current !== pending || gesture.current) return;
      const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
      element.scrollTo({
        left: swipeWidth,
        behavior: reduced ? "instant" : "smooth",
      });
      completeReturn(element);
    });
  }

  function release(
    event: JSX.TargetedTouchEvent<HTMLDivElement>,
    completed: boolean,
  ) {
    const start = gesture.current;
    const element = event.currentTarget;
    if (completed && start && event.touches.length) {
      start.eligible = false;
      // A finger on another row will end its touch outside this track.
      if (
        Array.from(event.touches).some((touch) =>
          element.contains(touch.target as Node),
        )
      )
        return;
    }
    gesture.current = undefined;
    element.removeAttribute("data-chat-swipe-dragging");
    if (!settling.current && Math.abs(element.scrollLeft - swipeWidth) < 1)
      return;
    const action =
      completed && start?.eligible && start.horizontal
        ? readyAction(element)
        : undefined;
    returnToCenter(element, action);
  }

  const icon = (action: SwipeAction) => (
    <span class="chat-swipe-action" data-action={action} aria-hidden="true">
      <ChatActionIcon kind={action} />
    </span>
  );
  return (
    <div
      class="chat-swipe-track"
      data-chat-swipe={canRead ? "read" : "unread"}
      style={{ "--chat-swipe-width": `${swipeWidth}px` }}
      ref={track}
      onTouchStart={(event) => {
        const touch = event.touches[0];
        const activeGesture = gesture.current;
        if (activeGesture && event.touches.length > 1) {
          activeGesture.eligible = false;
          if (!settling.current)
            event.currentTarget.removeAttribute("data-chat-swipe-ready");
          return;
        }
        suppressClick.current = Boolean(settling.current);
        gesture.current = touch
          ? {
              x: touch.clientX,
              y: touch.clientY,
              horizontal: false,
              eligible:
                enabled &&
                !settling.current &&
                event.touches.length === 1 &&
                canSwipeInPage(touch.clientX),
            }
          : undefined;
      }}
      onTouchMove={(event) => {
        const start = gesture.current;
        const touch = event.touches[0];
        if (!start || !touch) return;
        const x = Math.abs(touch.clientX - start.x);
        const y = Math.abs(touch.clientY - start.y);
        if (x > 10 && x > y) {
          start.horizontal = true;
          suppressClick.current = true;
          event.currentTarget.setAttribute("data-chat-swipe-dragging", "");
        }
      }}
      onScroll={(event) => {
        const element = event.currentTarget;
        const start = gesture.current;
        if (!start) {
          if (settling.current) completeReturn(element);
          // A late native scroll can arrive after the touch or menu closes.
          else if (
            element.clientWidth &&
            Math.abs(element.scrollLeft - swipeWidth) >= 1
          )
            returnToCenter(element);
          return;
        }
        if (!start.eligible || !start.horizontal) return;
        const action = readyAction(element);
        if (action) element.dataset.chatSwipeReady = action;
        else element.removeAttribute("data-chat-swipe-ready");
      }}
      onTouchEnd={(event) => release(event, true)}
      onTouchCancel={(event) => release(event, false)}
      onClickCapture={(event) => {
        if (!event.detail || !suppressClick.current) return;
        suppressClick.current = false;
        event.preventDefault();
        event.stopPropagation();
      }}
    >
      {icon("read")}
      {children}
      {icon("unread")}
    </div>
  );
}
