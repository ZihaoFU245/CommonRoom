import type { Snapshot } from "../../api/protocol.ts";
import { messageNotificationEvents } from "../conversation/interactions.ts";

interface NotificationOptions {
  baseline: boolean;
  enabled: boolean;
  NotificationClass: typeof Notification | undefined;
  background: boolean;
  selected: string;
  open: (view: string) => void;
}
export async function toggleMessageNotifications(
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

export function notifyMessages(
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
  for (const { message, view } of messageNotificationEvents(previous, next)) {
    if (!background && selected === view) continue;
    try {
      const direct = message.to === next.username;
      const preview = Array.from(message.text).slice(0, 200).join("");
      const notification = new NotificationClass(
        direct ? message.from : `#${view}`,
        {
          body: direct ? preview : `${message.from} mentioned you\n${preview}`,
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
