import { useEffect, useState } from "preact/hooks";
import {
  defaultFonts,
  loadFonts,
  saveFonts,
} from "../features/preferences/settings.ts";
export function useFonts() {
  const [fonts, setFonts] = useState(() => {
    try {
      return loadFonts(localStorage);
    } catch {
      return { ...defaultFonts };
    }
  });
  useEffect(() => {
    document.documentElement.style.setProperty(
      "--chat-font-size",
      `${fonts.chat}px`,
    );
    document.documentElement.style.setProperty(
      "--ui-font-size",
      `${fonts.ui}px`,
    );
    try {
      saveFonts(localStorage, fonts);
    } catch {
      /* Preferences also work without storage. */
    }
  }, [fonts]);
  return [fonts, setFonts] as const;
}
