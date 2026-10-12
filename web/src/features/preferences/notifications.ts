import type { Snapshot } from "../../api/protocol.ts";
interface NotificationOptions {
  baseline: boolean;
  enabled: boolean;
  NotificationClass: typeof Notification | undefined;
  background: boolean;
  selected: string;
  open: (view: string) => void;
}
import { mentionEvents } from "../conversation/interactions.ts";
export async function toggleMentionNotifications(
  enabled: boolean,
  NotificationClass: typeof Notification | undefined,
  secureContext: boolean,
): Promise<boolean> {
  if (enabled) return false;
  if (!NotificationClass || !secureContext)
    throw new Error(
      "Desktop notifications require a supported browser and HTTPS (or localhost).",
    );
  const permission =
    NotificationClass.permission === "default"
      ? await NotificationClass.requestPermission()
      : NotificationClass.permission;
  if (permission !== "granted")
    throw new Error(
      permission === "denied"
        ? "Notifications are blocked. Allow notifications in your browser's site settings, then try again."
        : "Notification permission was not granted. Notifications remain muted.",
    );
  return true;
}

export function notifyMentions(
  previous: Snapshot,
  next: Snapshot,
  {
    baseline,
    enabled,
    NotificationClass,
    background,
    selected,
    open,
  }: NotificationOptions,
) {
  if (baseline || !enabled || NotificationClass?.permission !== "granted")
    return 0;
  let delivered = 0;
  for (const { message, view } of mentionEvents(previous, next)) {
    if (!background && selected === view) continue;
    try {
      const notification = new NotificationClass(
        `${message.from} mentioned you`,
        {
          body: Array.from(message.text).slice(0, 200).join(""),
          tag: message.id,
        },
      );
      notification.onclick = () => {
        open(view);
        notification.close();
      };
      delivered++;
    } catch {
      /* A notification failure must never interrupt chat delivery. */
    }
  }
  return delivered;
}
