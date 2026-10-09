import { badgeLabel } from "../features/conversation/unread.ts";
export function UnreadBadge({ count = 0 }: { count?: number }) {
  return count > 0 ? (
    <span class="unread-badge" aria-label={`${count} unread messages`}>
      {badgeLabel(count)}
    </span>
  ) : null;
}
