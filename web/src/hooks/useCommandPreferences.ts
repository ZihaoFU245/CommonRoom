import { useAccountPreference } from "./useAccountPreference.ts";
import { useState } from "preact/hooks";
import {
  loadMarkdown,
  markdownManual,
} from "../features/preferences/markdown.ts";
import {
  loadAliases,
  saveAlias,
  setAlias,
  removeAlias,
} from "../features/conversation/aliases.ts";

export function useCommandPreferences(
  append: (command: string, result: string, error?: boolean) => string,
) {
  const [markdownValue, updateMarkdown] = useAccountPreference("markdown", () =>
    String(loadMarkdown()),
  );
  const markdown = markdownValue === "true";
  function changeMarkdown(enabled: boolean) {
    updateMarkdown(String(enabled));
  }
  const [aliases, setAliases] = useState(loadAliases);
  function saveAliasMapping(name: string, target: string) {
    setAliases(setAlias(aliases, name, target));
  }
  function removeAliasMapping(name: string) {
    setAliases(removeAlias(aliases, name));
  }
  function handleLocalCommand(text: string) {
    if (text.split(/\s+/u)[0] === "/version") {
      if (text !== "/version") {
        append(text, "Usage: /version", true);
        return "error";
      }
      append(
        text,
        `CommonRoom UI ${import.meta.env.CHAT_VERSION} (commit ${import.meta.env.CHAT_COMMIT})`,
      );
      return "done";
    }
    if (text.split(/\s+/u)[0] === "/markdown") {
      try {
        const normalized = text.replace(/\s+/gu, " ");
        if (!/^\/markdown (on|off)$/u.test(normalized))
          throw new Error("Usage: /markdown on|off");
        const enabled = normalized === "/markdown on";
        updateMarkdown(String(enabled));
        append(text, `Markdown ${enabled ? "on" : "off"} for future messages.`);
      } catch (error: unknown) {
        append(
          text,
          error instanceof Error ? error.message : "Could not update Markdown.",
          true,
        );
        return "error";
      }
      return "done";
    }
    if (/^\/man\s+\/?markdown$/u.test(text)) {
      append(text, markdownManual);
      return "done";
    }
    if (text.split(/\s+/u)[0] === "/alias") {
      try {
        setAliases(saveAlias(aliases, text));
        append(text, "Alias saved in this browser.");
      } catch (error: unknown) {
        append(
          text,
          error instanceof Error ? error.message : "Alias could not be saved.",
          true,
        );
        return "error";
      }
      return "done";
    }
    return null;
  }
  return {
    markdown,
    changeMarkdown,
    aliases,
    saveAliasMapping,
    removeAliasMapping,
    handleLocalCommand,
  };
}
