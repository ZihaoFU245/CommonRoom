import { useState } from "preact/hooks";
import {
  loadMarkdown,
  setMarkdown,
  saveMarkdown,
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
  const [markdown, updateMarkdown] = useState(loadMarkdown);
  function changeMarkdown(enabled: boolean) {
    updateMarkdown(saveMarkdown(enabled));
  }
  const [aliases, setAliases] = useState(loadAliases);
  function saveAliasMapping(name: string, target: string) {
    setAliases(setAlias(aliases, name, target));
  }
  function removeAliasMapping(name: string) {
    setAliases(removeAlias(aliases, name));
  }
  function handleLocalCommand(text: string) {
    if (text.split(/\s+/u)[0] === "/markdown") {
      try {
        const enabled = setMarkdown(text.replace(/\s+/gu, " "));
        updateMarkdown(enabled);
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
