import type { ChatNotification } from "../../api/protocol.ts";
import { notificationTagPrefix } from "./pushPayload.ts";

interface NotificationOptions {
  enabled: boolean;
  NotificationClass: typeof Notification | undefined;
  background: boolean;
  selected: string;
  open: (view: string) => void;
  worker?: Promise<ServiceWorkerRegistration> | undefined;
}
export function showChatNotification(
  { id, view, title, body }: ChatNotification,
  {
    enabled,
    NotificationClass,
    background,
    selected,
    open,
    worker,
  }: NotificationOptions,
) {
  if (
    !enabled ||
    NotificationClass?.permission !== "granted" ||
    (!background && selected === view)
  )
    return;
  try {
    const notification = new NotificationClass(title, { body, tag: id });
    notification.onclick = () => {
      open(view);
      notification.close();
    };
  } catch {
    worker
      ?.then((registration) =>
        registration.showNotification(title, {
          body,
          tag: notificationTagPrefix + id,
          data: { id, view },
        }),
      )
      .catch(() => {});
  }
}
