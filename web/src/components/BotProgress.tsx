import { useEffect, useState } from "preact/hooks";
import type { BotActivity } from "../api/protocol.ts";

/** Provisional plain text never enters durable history, unread state or Markdown. */
export function BotProgress({ activities }: { activities: BotActivity[] }) {
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
  if (!live.length) return null;
  return (
    <div class="bot-progress" role="status">
      {live.map((item) => (
        <div key={item.account_id}>
          <small>
            {item.draft ? "Draft · generating…" : "Bot is working…"}
          </small>
          {item.draft && <p dir="auto">{item.draft}</p>}
        </div>
      ))}
    </div>
  );
}
