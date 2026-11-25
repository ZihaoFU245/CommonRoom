import { useRef } from "preact/hooks";
import { api } from "../api/client.ts";
import { errorMessage } from "../api/protocol.ts";
import { useAccountPreference } from "./useAccountPreference.ts";
import { receivePreferences } from "../features/preferences/account.ts";

export function useChatNotifications(
  append: (command: string, result: string, error: boolean) => string,
) {
  const [muted] = useAccountPreference("notifications_muted", "");
  const [legacy] = useAccountPreference("notifications", "true");
  const notifications = muted ? muted !== "true" : legacy !== "false";
  const notificationsRef = useRef(notifications);
  notificationsRef.current = notifications;
  const pending = useRef<Promise<void> | null>(null);

  async function setMuted(value: boolean): Promise<void> {
    if (pending.current)
      throw new Error("A notification change is already in progress.");
    const save = api
      .preferences({ notifications_muted: String(value) }, false)
      .then((result) => {
        receivePreferences(result);
        notificationsRef.current = result.values.notifications_muted !== "true";
      });
    pending.current = save;
    try {
      if (
        !value &&
        window.isSecureContext &&
        window.Notification?.permission === "default"
      )
        window.Notification.requestPermission().catch((error: unknown) => {
          append("Notifications", errorMessage(error), true);
        });
      await save;
    } finally {
      pending.current = null;
    }
  }

  async function toggleNotifications() {
    try {
      await setMuted(notificationsRef.current);
    } catch (error) {
      append("Notifications", errorMessage(error), true);
    }
  }

  return { notifications, notificationsRef, toggleNotifications, setMuted };
}
