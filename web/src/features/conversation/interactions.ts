import type { Hint } from "../../api/protocol.ts";
function nameCharacter(value: string): boolean {
  return (
    /^[A-Za-z0-9_-]$/u.test(value) ||
    ((value.codePointAt(0) ?? 0) > 127 &&
      !/[\p{White_Space}\p{Cc}]/u.test(value))
  );
}
export function mentionSuggestions(
  draft: string,
  users: string[],
  canMentionAll = false,
): Hint[] {
  if (draft.startsWith("/")) return [];
  const index = draft.lastIndexOf("@");
  if (index < 0) return [];
  const preceding = Array.from(draft.slice(0, index)).at(-1);
  if (preceding && (nameCharacter(preceding) || preceding === "@")) return [];
  const prefix = draft.slice(index + 1);
  if (!Array.from(prefix).every(nameCharacter)) return [];
  const candidates = canMentionAll ? ["all", ...users] : users;
  return candidates
    .filter((name) => name.startsWith(prefix))
    .map((name) => ({
      label: `@${name}`,
      description:
        canMentionAll && name === "all"
          ? "Mention everyone in this room"
          : "Mention",
      value: draft.slice(0, index) + `@${name} `,
    }));
}
export function mentionedText(
  text: string,
  mentions: string[] = [],
  mentionAll = false,
) {
  const names = new Set(mentions);
  if (mentionAll) names.add("all");
  return text
    .split(
      /(@(?:(?![\p{White_Space}\p{Cc}])[A-Za-z0-9_\-\u0080-\u{10ffff}])+)/u,
    )
    .map((part, index, parts) => ({
      text: part,
      mention:
        part.startsWith("@") &&
        names.has(part.slice(1)) &&
        (!index ||
          !Array.from(parts[index - 1] || "")
            .slice(-1)
            .some((value) => nameCharacter(value) || value === "@")),
    }));
}

/** Keep addressed slash commands and their arguments literal in the transcript. */
export function mentionedCommand(text: string, mentions: string[] = []) {
  const match = /^(@\S+)(\s+)(\/\S+[\s\S]*)$/u.exec(text);
  const mention = match?.[1];
  const separator = match?.[2];
  const command = match?.[3];
  return mention && separator && command && mentions.includes(mention.slice(1))
    ? { mention, separator, command }
    : null;
}
