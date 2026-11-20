import { useState } from "preact/hooks";
import { readStorage, writeStorage } from "../api/browserStorage.ts";
import { serverBaseUrl } from "../api/server.ts";
import { useFonts } from "./useFonts.ts";
import { useAccountPreference } from "./useAccountPreference.ts";
import {
  loadLatex,
  loadNameColors,
  loadMessageBars,
  loadMessageBarOpacity,
  loadNameBorders,
} from "../features/preferences/formatting.ts";
import { loadMarkdownHtml } from "../features/preferences/security.ts";

const enterSendKey = "commonroom-always-send-with-enter";

function useBoolean(key: string, load: () => boolean) {
  const [value, set] = useAccountPreference(key, () => String(load()));
  return [value === "true", (next: boolean) => set(String(next))] as const;
}
export function useChatPreferences() {
  const [fonts, setFonts] = useFonts();
  const [alwaysSendWithEnter, setAlwaysSendWithEnter] = useState(
    () => readStorage(enterSendKey) === "true",
  );
  const [latex, setLatex] = useBoolean("latex", loadLatex);
  const [showNameColors, setShowNameColors] = useBoolean(
    "name_colors",
    loadNameColors,
  );
  const [messageBars, setMessageBars] = useBoolean(
    "message_bars",
    loadMessageBars,
  );
  const [nameBorders, setNameBorders] = useBoolean(
    "name_borders",
    loadNameBorders,
  );
  const [allowHtml, setAllowHtml] = useBoolean("allow_html", loadMarkdownHtml);
  const [opacity, setOpacity] = useAccountPreference(
    "message_bar_opacity",
    () => String(loadMessageBarOpacity()),
  );
  return {
    fonts,
    setFonts,
    alwaysSendWithEnter,
    setAlwaysSendWithEnter: (enabled: boolean) => {
      setAlwaysSendWithEnter(enabled);
      writeStorage(enterSendKey, String(enabled));
    },
    latex,
    setLatex,
    showNameColors,
    setShowNameColors,
    messageBars,
    setMessageBars,
    messageBarOpacity: Number(opacity),
    setMessageBarOpacity: (value: number) => setOpacity(String(value)),
    nameBorders,
    setNameBorders,
    allowHtml,
    setAllowHtml,
  };
}
export function useBotStatus(account: string, supportsSync: boolean) {
  const storageKey = `commonroom-bot-status:${serverBaseUrl()}${account}`;
  const [local, setLocal] = useState(() => readStorage(storageKey) !== "false");
  const [synced, setSynced] = useAccountPreference("bot_status", "true");
  const set = (enabled: boolean) => {
    // Older backends reject the entire preference batch if this key is present.
    if (supportsSync) setSynced(String(enabled));
    else {
      setLocal(enabled);
      writeStorage(storageKey, String(enabled));
    }
  };
  return [supportsSync ? synced === "true" : local, set] as const;
}
