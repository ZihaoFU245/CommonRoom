import type { Unread } from "../../api/protocol.ts";

export interface ConversationEntry {
  view: string;
  kind: "room" | "direct";
  /** Displayed room name or pair, or the peer's username for direct views. */
  name: string;
  /** Latest global message sequence; 0 when the conversation has no messages. */
  latest: number;
}

/**
 * Orders rooms and private conversations as one list. Unsent private drafts
 * come first (newest draft first), then conversations by their latest global
 * message sequence, then conversations without messages by name.
 */
export function conversationList(
  rooms: readonly { name: string }[],
  peers: readonly string[],
  drafts: readonly string[],
  unread: Readonly<Record<string, Unread>>,
): ConversationEntry[] {
  const entries: ConversationEntry[] = [
    ...rooms.map(({ name }) => ({
      view: name,
      kind: "room" as const,
      name: name.startsWith("@private:") ? name.slice(9) : name,
      latest: unread[name]?.through ?? 0,
    })),
    ...peers.map((name) => ({
      view: `@direct:${name}`,
      kind: "direct" as const,
      name,
      latest: unread[`@direct:${name}`]?.through ?? 0,
    })),
  ];
  const draft = (entry: ConversationEntry) =>
    entry.kind === "direct" && entry.latest === 0
      ? drafts.lastIndexOf(entry.name)
      : -1;
  return entries.sort(
    (a, b) =>
      draft(b) - draft(a) ||
      b.latest - a.latest ||
      a.name.localeCompare(b.name) ||
      a.view.localeCompare(b.view),
  );
}
