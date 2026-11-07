import { useLayoutEffect, useMemo, useState } from "preact/hooks";
import { useAccountPreference } from "./useAccountPreference.ts";
import type { Fonts } from "../api/protocol.ts";
import { loadFonts } from "../features/preferences/settings.ts";
export function useFonts() {
  const [local] = useState(loadFonts);
  const [chat, setChat] = useAccountPreference("font.chat", String(local.chat));
  const [ui, setUi] = useAccountPreference("font.ui", String(local.ui));
  const fonts = useMemo(
    () => ({ chat: Number(chat), ui: Number(ui) }),
    [chat, ui],
  );
  useLayoutEffect(() => {
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
