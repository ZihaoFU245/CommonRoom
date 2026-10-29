import { useEffect, useRef, useState } from "preact/hooks";
import { errorMessage } from "../api/protocol.ts";
import { toggleMentionNotifications } from "../features/preferences/notifications.ts";

export function useChatNotifications(
  append: (command: string, result: string, error: boolean) => string,
) {
  const [notifications, setNotifications] = useState(() => {
    try {
      return (
        typeof Notification !== "undefined" &&
        Notification.permission === "granted" &&
        localStorage.getItem("commonroom-notifications") === "on"
      );
    } catch {
      return false;
    }
  });
  const notificationsRef = useRef(notifications);
  const notificationRequest = useRef(false);
  useEffect(() => {
    const checkPermission = () => {
      if (window.Notification?.permission !== "granted") {
        notificationsRef.current = false;
        setNotifications(false);
      }
    };
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
      notificationsRef.current = enabled;
      setNotifications(enabled);
      try {
        localStorage.setItem(
          "commonroom-notifications",
          enabled ? "on" : "off",
        );
      } catch {
        // The toggle still works for this session without browser storage.
      }
    } catch (error) {
      append("Notifications", errorMessage(error), true);
    } finally {
      notificationRequest.current = false;
    }
  }

  return { notifications, notificationsRef, toggleNotifications };
}
