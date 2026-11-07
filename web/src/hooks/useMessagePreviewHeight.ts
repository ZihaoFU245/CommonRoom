import { useAccountPreference } from "./useAccountPreference.ts";
import {
  messagePreviewMin,
  normalizeMessagePreviewHeight,
} from "../features/preferences/messagePreview.ts";

export function useMessagePreviewHeight() {
  const [value, set] = useAccountPreference(
    "message_preview_height",
    String(messagePreviewMin),
  );
  return [
    normalizeMessagePreviewHeight(Number(value)),
    (height: number) => set(String(normalizeMessagePreviewHeight(height))),
  ] as const;
}
