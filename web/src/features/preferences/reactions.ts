export const reactionChoices = [
  "🍼",
  "🎉",
  "😹",
  "👌",
  "😂",
  "❔",
  "🔥",
  "🔞",
  "⚡️",
  "😭",
  "❤️",
  "✍️",
  "🤔",
  "⏰",
  "🗿",
  "👀",
  "💀",
  "🍅",
  "👎",
  "👍",
  "💩",
  "👽",
  "🖕",
];
const key = "commonroom.recentReactions";
const recentLimit = 24;

/** Recently used reactions first, then the remaining defaults in order. */
export function orderReactions(recent: readonly string[]): string[] {
  return [...new Set([...recent, ...reactionChoices])];
}

export function rememberReaction(
  recent: readonly string[],
  value: string,
): string[] {
  const reaction = value.trim();
  if (!reaction) return [...recent];
  return [reaction, ...recent.filter((item) => item !== reaction)].slice(
    0,
    recentLimit,
  );
}

export function loadRecentReactions(): string[] {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(key) ?? "[]");
    if (!Array.isArray(stored)) return [];
    return stored
      .filter((item): item is string => typeof item === "string" && !!item)
      .slice(0, recentLimit);
  } catch {
    return [];
  }
}

export function saveRecentReaction(value: string): void {
  try {
    localStorage.setItem(
      key,
      JSON.stringify(rememberReaction(loadRecentReactions(), value)),
    );
  } catch {
    /* Keep the default order when storage is unavailable. */
  }
}
