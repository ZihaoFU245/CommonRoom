import { readStorage, writeStorage } from "../../api/browserStorage.ts";
const htmlKey = "commonroom-markdown-html";
export const loadMarkdownHtml = () => readStorage(htmlKey) === "on";
export const saveMarkdownHtml = (enabled: boolean) => {
  writeStorage(htmlKey, enabled ? "on" : "off");
};
