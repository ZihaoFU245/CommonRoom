import { useState } from "preact/hooks";
import { useEvent } from "./useEvent.ts";
import { useFonts } from "./useFonts.ts";
import {
  loadMarkdownHtml,
  saveMarkdownHtml,
} from "../features/preferences/security.ts";
import {
  loadLatex,
  loadMessageBars,
  loadMessageBarOpacity,
  loadNameColors,
  loadNameBorders,
  saveLatex,
  saveMessageBars,
  saveMessageBarOpacity,
  saveNameColors,
  saveNameBorders,
} from "../features/preferences/formatting.ts";

function usePreference<T>(load: () => T, save: (value: T) => void) {
  const [value, setValue] = useState(load);
  const change = useEvent((next: T) => {
    setValue(next);
    save(next);
  });
  return [value, change] as const;
}
export function useChatPreferences() {
  const [fonts, setFonts] = useFonts();
  const [latex, setLatex] = usePreference(loadLatex, saveLatex);
  const [showNameColors, setShowNameColors] = usePreference(
    loadNameColors,
    saveNameColors,
  );
  const [messageBars, setMessageBars] = usePreference(
    loadMessageBars,
    saveMessageBars,
  );
  const [messageBarOpacity, setMessageBarOpacity] = usePreference(
    loadMessageBarOpacity,
    saveMessageBarOpacity,
  );
  const [nameBorders, setNameBorders] = usePreference(
    loadNameBorders,
    saveNameBorders,
  );
  const [allowHtml, setAllowHtml] = usePreference(
    loadMarkdownHtml,
    saveMarkdownHtml,
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
    messageBarOpacity,
    setMessageBarOpacity,
    nameBorders,
    setNameBorders,
    allowHtml,
    setAllowHtml,
  };
}
