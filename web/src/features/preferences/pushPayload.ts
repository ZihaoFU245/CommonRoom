export interface PushPayload {
  view: string;
  id: string;
  title?: string;
  body?: string;
}

export const notificationTagPrefix = "commonroom-push:";
// The server can emit 200 supplementary code points plus a truncation ellipsis.
export const notificationPreviewMaxLength = 401;

export function isPushPayload(value: unknown): value is PushPayload {
  return (
    typeof value === "object" &&
    value !== null &&
    "view" in value &&
    typeof value.view === "string" &&
    value.view.length > 0 &&
    value.view.length <= 256 &&
    "id" in value &&
    typeof value.id === "string" &&
    /^[\w-]{1,128}$/u.test(value.id) &&
    (!("title" in value) ||
      (typeof value.title === "string" && value.title.length <= 512)) &&
    (!("body" in value) ||
      (typeof value.body === "string" &&
        value.body.length <= notificationPreviewMaxLength))
  );
}
