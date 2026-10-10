import type { Message, Snapshot, Hint } from "../../api/protocol.ts";
function nameCharacter(value: string): boolean {
  return (
    /^[A-Za-z0-9_-]$/u.test(value) ||
    ((value.codePointAt(0) ?? 0) > 127 &&
      !/[\p{White_Space}\p{Cc}]/u.test(value))
  );
}
export function mentionSuggestions(draft: string, users: string[]): Hint[] {
  if (draft.startsWith("/")) return [];
  const index = draft.lastIndexOf("@");
  if (index < 0) return [];
  const preceding = Array.from(draft.slice(0, index)).at(-1);
  if (preceding && (nameCharacter(preceding) || preceding === "@")) return [];
  const prefix = draft.slice(index + 1);
  if (!Array.from(prefix).every(nameCharacter)) return [];
  return users
    .filter((name) => name.startsWith(prefix))
    .map((name) => ({
      label: `@${name}`,
      description: "Mention",
      value: draft.slice(0, draft.lastIndexOf("@")) + `@${name} `,
    }));
}
export function mentionEvents(previous: Snapshot, next: Snapshot) {
  const seen = new Set(
    [...previous.rooms.flatMap((r) => r.messages), ...previous.direct].map(
      (m) => m.id,
    ),
  );
  const previousRooms = new Map(previous.rooms.map((r) => [r.name, r]));
  const events: { message: Message; view: string | null }[] = [];
  function add(messages: Message[], older: Message[], view: string | null) {
    const newest = Math.max(0, ...older.map((m) => m.time));
    for (const message of messages) {
      if (
        !seen.has(message.id) &&
        message.time >= newest &&
        message.from !== next.username &&
        message.mentions?.includes(next.username)
      )
        events.push({ message, view });
    }
  }
  for (const room of next.rooms) {
    const previousRoom = previousRooms.get(room.name);
    if (previousRoom) add(room.messages, previousRoom.messages, room.name);
  }
  add(next.direct, previous.direct, null);
  return events.map((e) => ({
    ...e,
    view: e.view || `@direct:${e.message.from}`,
  }));
}
export function mentionedText(text: string, mentions: string[] = []) {
  const names = new Set(mentions);
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
