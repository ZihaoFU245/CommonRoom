import type { Snapshot } from "../../api/protocol.ts";

export type NotificationScopes = Pick<
  Snapshot,
  "rooms" | "private_access" | "preferences"
>;

export const globalNotificationFilter = "notification.global";

export const defaultMatchers = [
  { key: "notification_default_mentions_me", label: "Direct mentions" },
  { key: "notification_default_mentions_all", label: "Room-wide mentions" },
  { key: "notification_default_replies", label: "Replies to you" },
  { key: "notification_default_direct", label: "Incoming DMs" },
] as const;

export const notificationDefaultOptions = [
  ...defaultMatchers,
  {
    key: "notification_exclude_bots",
    label: "Exclude unthreaded bot messages",
  },
  {
    key: "notification_exclude_bot_mentions",
    label: "Exclude messages that mention bots",
  },
] as const;

export function notificationScopes({
  rooms,
  private_access,
}: NotificationScopes) {
  return [
    ...rooms
      .filter(({ name }) => !name.startsWith("@"))
      .map(({ id, name }) => ({ id, label: `#${name}`, direct: false })),
    ...Object.entries(private_access).map(([view, { id }]) => ({
      id,
      label: view,
      direct: true,
    })),
  ].sort((left, right) => left.label.localeCompare(right.label));
}
