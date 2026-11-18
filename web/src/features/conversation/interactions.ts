import type { Message, Snapshot, Hint } from "../../api/protocol.ts";
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
export function messageNotificationEvents(previous: Snapshot, next: Snapshot) {
  const seen = new Set(
    [...previous.rooms.flatMap((r) => r.messages), ...previous.direct].map(
      (m) => m.id,
    ),
  );
  const previousRooms = new Map(previous.rooms.map((r) => [r.name, r]));
  const events: { message: Message; view: string; roomWide?: true }[] = [];
  for (const room of next.rooms) {
    const previousRoom = previousRooms.get(room.name);
    if (!previousRoom) continue;
    const newest = Math.max(0, ...previousRoom.messages.map((m) => m.time));
    const through = previous.unread[room.name]?.through;
    for (const message of room.messages) {
      const roomMention =
        message.mention_all &&
        !message.edited &&
        room.is_member &&
        previousRoom.id === room.id &&
        through !== undefined &&
        message.sequence > through;
      if (
        !seen.has(message.id) &&
        message.from !== next.username &&
        (roomMention ||
          (message.time >= newest && message.mentions?.includes(next.username)))
      )
        events.push({
          message,
          view: room.name,
          ...(roomMention ? { roomWide: true } : {}),
        });
    }
  }
  for (const message of next.direct) {
    if (
      message.from === next.username ||
      message.to !== next.username ||
      seen.has(message.id)
    )
      continue;
    const view = `@direct:${message.from}`;
    if (message.sequence > (previous.unread[view]?.through ?? 0))
      events.push({ message, view });
  }
  return events;
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
