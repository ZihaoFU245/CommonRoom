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

function useBoolean(key: string, load: () => boolean) {
  const [value, set] = useAccountPreference(key, () => String(load()));
  return [value === "true", (next: boolean) => set(String(next))] as const;
}
export function useChatPreferences() {
  const [fonts, setFonts] = useFonts();
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
