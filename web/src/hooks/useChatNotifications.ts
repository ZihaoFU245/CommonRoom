import { useEffect, useRef, useState } from "preact/hooks";
import { useAccountPreference } from "./useAccountPreference.ts";
import { errorMessage } from "../api/protocol.ts";
import { toggleMentionNotifications } from "../features/preferences/notifications.ts";

export function useChatNotifications(
  append: (command: string, result: string, error: boolean) => string,
) {
  const [desired, setDesired] = useAccountPreference("notifications", () => {
    try {
      return String(localStorage.getItem("commonroom-notifications") === "on");
    } catch {
      return "false";
    }
  });
  const [permission, setPermission] = useState(
    () =>
      typeof Notification !== "undefined" &&
      Notification.permission === "granted",
  );
  const notifications = desired === "true" && permission;
  const notificationsRef = useRef(notifications);
  notificationsRef.current = notifications;
  const notificationRequest = useRef(false);
  useEffect(() => {
    const checkPermission = () =>
      setPermission(window.Notification?.permission === "granted");
    window.addEventListener("focus", checkPermission);
    return () => window.removeEventListener("focus", checkPermission);
  }, []);
  async function toggleNotifications() {
    if (notificationRequest.current) return;
    notificationRequest.current = true;
    try {
      const enabled = await toggleMentionNotifications(
        notificationsRef.current,
        window.Notification,
        window.isSecureContext,
      );
      setPermission(window.Notification?.permission === "granted");
      setDesired(String(enabled));
      notificationsRef.current = enabled;
    } catch (error) {
      append("Notifications", errorMessage(error), true);
    } finally {
      notificationRequest.current = false;
    }
  }
  return { notifications, notificationsRef, toggleNotifications };
}
