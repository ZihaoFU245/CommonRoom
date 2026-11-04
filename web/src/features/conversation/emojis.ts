import { readStorageJson, writeStorage } from "../../api/browserStorage.ts";
import { emojiData } from "./emojiData.ts";

const catalogue = new Map(emojiData.map((item) => [item[1], item]));

export function recentEmoji(key: string): string[] {
  const value = readStorageJson(key);
  return Array.isArray(value)
    ? [
        ...new Set(
          value.filter(
            (item): item is string =>
              typeof item === "string" && catalogue.has(item),
          ),
        ),
      ].slice(0, 50)
    : [];
}

export function rememberEmoji(key: string, emoji: string, recent: string[]) {
  const next = [emoji, ...recent.filter((item) => item !== emoji)].slice(0, 50);
  writeStorage(key, JSON.stringify(next));
  return next;
}

export function matchingEmoji(
  category: number,
  query: string,
  recent: string[],
) {
  const words = query.toLowerCase().trim().split(/\s+/u).filter(Boolean);
  if (words.length)
    return emojiData.filter(([, emoji, name, keywords]) =>
      words.every((word) =>
        `${emoji} ${name} ${keywords}`.toLowerCase().includes(word),
      ),
    );
  return category === 0
    ? recent.flatMap((emoji) => {
        const item = catalogue.get(emoji);
        return item ? [item] : [];
      })
    : emojiData.filter(([group]) => group === category);
}
