import type { ComponentChildren } from "preact";
import { createPortal } from "preact/compat";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { useEvent } from "../hooks/useEvent.ts";
import { controlMotion } from "../features/preferences/motion.ts";
import type { Snapshot } from "../api/protocol.ts";
import { UserCardContext } from "./UserCardTrigger.tsx";
import { Avatar } from "./Avatar.tsx";

function CopyUsername({ name }: { name: string }) {
  const [feedback, setFeedback] = useState("");
  useEffect(() => {
    if (!feedback) return;
    const timer = window.setTimeout(() => setFeedback(""), 1600);
    return () => window.clearTimeout(timer);
  }, [feedback]);
  return (
    <>
      <button
        type="button"
        class="user-card-username"
        aria-label={`Copy @${name}`}
        title="Copy username"
        onClick={() => {
          if (!navigator.clipboard?.writeText) {
            setFeedback("Copy unavailable");
            return;
          }
          navigator.clipboard.writeText(`@${name}`).then(
            () => setFeedback("Copied"),
            () => setFeedback("Couldn’t copy"),
          );
        }}
      >
        {feedback || `@${name}`}
      </button>
      <span class="visually-hidden" role="status">
        {feedback}
      </span>
    </>
  );
}

export function UserCards({
  state,
  onMessage,
  children,
}: {
  state: Snapshot;
  onMessage: (name: string) => void;
  children: ComponentChildren;
}) {
  const [target, setTarget] = useState<{
    name: string;
    label: string;
    anchor: HTMLElement;
    origin: { x: number; y: number };
  } | null>(null);
  const card = useRef<HTMLDivElement>(null);
  const reveal = useRef<Animation | null>(null);
  const closing = useRef(false);
  const [position, setPosition] = useState({ left: 12, top: 12 });
  const animateDirection = useEvent((close: boolean) => {
    const animation = reveal.current;
    if (
      !animation ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches ||
      (close &&
        typeof animation.currentTime === "number" &&
        animation.currentTime <= 0)
    ) {
      setTarget(null);
      return;
    }
    closing.current = close;
    // Assign the direction immediately so rapid clicks don't queue pending reversals.
    animation.playbackRate = close ? -1 : 1;
    animation.play();
  });
  const closeCard = useEvent(() => {
    if (!closing.current) animateDirection(true);
  });
  useLayoutEffect(() => {
    if (!target || !card.current) return;
    const element = card.current;
    const reposition = () => {
      const anchor = target.anchor.getBoundingClientRect();
      const below = anchor.bottom + 8;
      const above = anchor.top - element.offsetHeight - 8;
      const preferredTop =
        below + element.offsetHeight <= window.innerHeight - 12 ? below : above;
      const next = {
        left: Math.max(
          12,
          Math.min(anchor.left, window.innerWidth - element.offsetWidth - 12),
        ),
        top: Math.max(
          12,
          Math.min(
            preferredTop,
            window.innerHeight - element.offsetHeight - 12,
          ),
        ),
      };
      setPosition(next);
      return next;
    };
    const initialPosition = reposition();
    const observer = new ResizeObserver(reposition);
    observer.observe(element);
    element.focus();
    closing.current = false;
    const animation = window.matchMedia("(prefers-reduced-motion: reduce)")
      .matches
      ? undefined
      : element.animate(
          [
            {
              opacity: 0,
              transform: "scale(0.15)",
              transformOrigin: `${target.origin.x - initialPosition.left}px ${target.origin.y - initialPosition.top}px`,
            },
            {
              opacity: 1,
              transform: "scale(1)",
              transformOrigin: `${target.origin.x - initialPosition.left}px ${target.origin.y - initialPosition.top}px`,
            },
          ],
          { ...controlMotion, duration: 90, fill: "both" },
        );
    reveal.current = animation ?? null;
    if (animation) {
      animation.onfinish = () => {
        if (
          closing.current &&
          reveal.current === animation &&
          animation.playState === "finished" &&
          animation.playbackRate < 0
        )
          setTarget(null);
      };
    }
    return () => {
      observer.disconnect();
      if (animation) {
        animation.onfinish = null;
        animation.cancel();
      }
      if (reveal.current === animation) reveal.current = null;
    };
  }, [target]);
  useEffect(() => {
    if (!target) return;
    const close = closeCard;
    const outside = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        !card.current?.contains(event.target) &&
        !target.anchor.contains(event.target)
      )
        close();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        close();
        target.anchor.focus();
      }
    };
    const focus = (event: FocusEvent) => {
      if (
        event.target instanceof Node &&
        !card.current?.contains(event.target) &&
        !target.anchor.contains(event.target)
      )
        close();
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", key, true);
    document.addEventListener("focusin", focus);
    window.addEventListener("resize", close);
    document.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", key, true);
      document.removeEventListener("focusin", focus);
      window.removeEventListener("resize", close);
      document.removeEventListener("scroll", close, true);
    };
  }, [target, closeCard]);
  return (
    <UserCardContext.Provider
      value={(name, label, anchor, origin) => {
        if (target?.anchor === anchor) {
          animateDirection(!closing.current);
          return;
        }
        setTarget({ name, label, anchor, origin });
      }}
    >
      {children}
      {target &&
        createPortal(
          <div
            ref={card}
            class="user-card"
            role="dialog"
            aria-label={`${target.label}'s profile`}
            tabIndex={-1}
            style={position}
          >
            <div class="user-card-avatar">
              <Avatar
                url={state.avatars?.[target.name]}
                name={target.name}
                large
              />
            </div>
            <div class="user-card-details">
              <div class="user-card-name">
                <span
                  class={`user-card-presence${state.online.includes(target.name) ? " online" : ""}`}
                  role="img"
                  aria-label={
                    state.online.includes(target.name) ? "Online" : "Offline"
                  }
                />
                <strong>{target.label}</strong>
                {target.name === state.username && (
                  <small class="user-card-self">You</small>
                )}
              </div>
              <CopyUsername key={target.name} name={target.name} />
              <small>
                Groups:{" "}
                {state.user_groups?.[target.name]?.join(", ") ||
                  (target.name === state.username
                    ? state.groups.join(", ")
                    : "Group unavailable")}
              </small>
            </div>
            {target.name !== state.username && (
              <button
                type="button"
                class="primary"
                onClick={() => {
                  setTarget(null);
                  onMessage(target.name);
                }}
              >
                Message
              </button>
            )}
          </div>,
          document.body,
        )}
    </UserCardContext.Provider>
  );
}
