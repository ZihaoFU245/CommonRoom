import { useEffect, useState } from "preact/hooks";
import type { BotActivity } from "../api/protocol.ts";
import { Avatar } from "./Avatar.tsx";
import { DisplayName } from "./DisplayName.tsx";
import { useNameColor } from "./NameColorContext.ts";

/**
 * Transient bot progress. Provisional text never enters durable history, unread
 * state or Markdown, and stays out of the live region so streaming does not
 * interrupt screen readers; the durable message is announced when it arrives.
 */
export function BotProgress({
  activities,
  avatars,
}: {
  activities: BotActivity[];
  avatars?: Record<string, string> | undefined;
}) {
  const [now, setNow] = useState(() => Date.now() / 1000);
  const live = activities.filter(
    (item) => item.expires > Math.max(now, Date.now() / 1000),
  );
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
      class={`bot-progress${live.length ? " live" : ""}`}
      role="status"
      aria-live="polite"
      aria-atomic="false"
    >
      {live.map((item) => (
        <BotActivityRow
          key={item.account_id}
          activity={item}
          avatars={avatars}
        />
      ))}
    </div>
  );
}

function BotActivityRow({
  activity,
  avatars,
}: {
  activity: BotActivity;
  avatars?: Record<string, string> | undefined;
}) {
  const name = activity.username?.trim() || "Bot";
  const color = useNameColor(name);
  const text = activity.draft?.trim();
  // Blank lines would spend a whole clamped preview line; keep the teaser dense.
  const draft = text ? text.replace(/\n[ \t]*\n+/gu, "\n") : undefined;
  return (
    <div class="bot-activity" data-draft={draft ? "true" : undefined}>
      <Avatar url={avatars?.[name]} name={name} />
      <div class="bot-activity-content">
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
          <div class="bot-activity-draft" dir="auto" aria-hidden="true">
            <p class="bot-activity-draft-text">{draft}</p>
          </div>
        )}
      </div>
    </div>
  );
}
