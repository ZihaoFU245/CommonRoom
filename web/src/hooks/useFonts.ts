import { useEffect, useMemo, useState } from "preact/hooks";
import { useAccountPreference } from "./useAccountPreference.ts";
import type { Fonts } from "../api/protocol.ts";
import { defaultFonts, loadFonts } from "../features/preferences/settings.ts";
export function useFonts() {
  const [local] = useState(() => {
    try {
      return loadFonts(localStorage);
    } catch {
      return defaultFonts;
    }
  });
  const [chat, setChat] = useAccountPreference("font.chat", String(local.chat));
  const [ui, setUi] = useAccountPreference("font.ui", String(local.ui));
  const fonts = useMemo(
    () => ({ chat: Number(chat), ui: Number(ui) }),
    [chat, ui],
  );
  useEffect(() => {
    document.documentElement.style.setProperty("--chat-font-size", `${chat}px`);
    document.documentElement.style.setProperty("--ui-font-size", `${ui}px`);
  }, [chat, ui]);
  return [
    fonts,
    (value: Fonts) => {
      setChat(String(value.chat));
      setUi(String(value.ui));
    },
  ] as const;
}
