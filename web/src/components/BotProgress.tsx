import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import type { BotActivity } from "../api/protocol.ts";
import { DisplayName } from "./DisplayName.tsx";
import { useNameColor } from "./NameColorContext.ts";

/**
 * Transient bot progress. Provisional text never enters durable history, unread
 * state or Markdown, and stays out of the live region so streaming does not
 * interrupt screen readers; the durable message is announced when it arrives.
 */
export function BotProgress({ activities }: { activities: BotActivity[] }) {
  const [now, setNow] = useState(() => Date.now() / 1000);
  const panel = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const [scrollable, setScrollable] = useState(false);
  const live = activities.filter(
    (item) => item.expires > Math.max(now, Date.now() / 1000),
  );
  useLayoutEffect(() => {
    const viewport = panel.current;
    const rows = content.current;
    if (!viewport || !rows) return;
    const measure = () =>
      setScrollable(viewport.scrollHeight > viewport.clientHeight);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(viewport);
    observer.observe(rows);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    const expiry = Math.min(
      ...activities
        .filter((item) => item.expires > now)
        .map((item) => item.expires),
    );
    if (!Number.isFinite(expiry)) return;
    const timer = setTimeout(
      () => setNow(Date.now() / 1000),
      Math.max(0, expiry * 1000 - Date.now()) + 1,
    );
    return () => clearTimeout(timer);
  }, [activities, now]);
  return (
    // The live region stays mounted empty so first activity is announced.
    <div
      ref={panel}
      class={`bot-progress${live.length ? " live" : ""}`}
      role="status"
      aria-label="Bot activity"
      aria-live="polite"
      aria-atomic="false"
      tabIndex={live.length && scrollable ? 0 : undefined}
    >
      <div ref={content}>
        {live.map((item) => (
          <BotActivityRow key={item.account_id} activity={item} />
        ))}
      </div>
    </div>
  );
}

function BotActivityRow({ activity }: { activity: BotActivity }) {
  const name = activity.username?.trim() || "Bot";
  const color = useNameColor(name);
  const draft = activity.draft?.trim();
  return (
    <div class="bot-activity">
      <div class="bot-activity-line">
        <strong class="bot-activity-name" data-name-color={color}>
          <DisplayName name={name} />
        </strong>
        <span class="bot-activity-state">
          {draft ? "is drafting" : "is working"}
        </span>
        <span class="bot-activity-dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
      </div>
      {draft && (
        <p class="bot-activity-draft" dir="auto" aria-hidden="true">
          {draft}
        </p>
      )}
    </div>
  );
}
