import type { History, Message, Snapshot } from "../../api/protocol.ts";
export const ROOM_CACHE_LIMIT = 1000;
const TOTAL_CACHE_LIMIT = 5000;
const BYTE_LIMIT = 16 * 1024 * 1024;
export interface CachedHistory extends History {
  id: string;
  through: number;
  oldest: number;
}
export function bytes(messages: Message[]) {
  // Conservative UTF-16 payload accounting; limits also bound object/entry counts.
  return messages.reduce(
    (sum, message) =>
      sum +
      512 +
      2 *
        (message.text.length +
          message.id.length +
          message.from.length +
          (message.reply?.text.length ?? 0) +
          message.mentions.join("").length +
          Object.entries(message.reactions).reduce(
            (n, [reaction, users]) =>
              n + reaction.length + users.join("").length,
            0,
          )),
    0,
  );
}
export function storeHistory(
  cache: Map<string, CachedHistory>,
  page: CachedHistory,
) {
  cache.delete(page.view);
  cache.set(page.view, page);
  let count = 0;
  let size = 0;
  for (const entry of cache.values()) {
    count += entry.messages.length;
    size += bytes(entry.messages);
  }
  while (count > TOTAL_CACHE_LIMIT || size > BYTE_LIMIT || cache.size > 32) {
    const oldest = cache.values().next().value;
    if (!oldest) break;
    count -= oldest.messages.length;
    size -= bytes(oldest.messages);
    cache.delete(oldest.view);
  }
}
export function mergePage(
  current: CachedHistory,
  page: History,
  direction: "older" | "newer",
): CachedHistory {
  const byId = new Map(
    [...current.messages, ...page.messages].map((message) => [
      message.id,
      message,
    ]),
  );
  const sorted = [...byId.values()].sort((a, b) => a.sequence - b.sequence);
  const removed = sorted.length > ROOM_CACHE_LIMIT;
  let messages =
    direction === "older"
      ? sorted.slice(0, ROOM_CACHE_LIMIT)
      : sorted.slice(-ROOM_CACHE_LIMIT);
  let byteEviction = false;
  while (bytes(messages) > 4 * 1024 * 1024 && messages.length > 1) {
    messages =
      direction === "older" ? messages.slice(0, -1) : messages.slice(1);
    byteEviction = true;
  }
  return {
    ...current,
    messages,
    revision: page.revision,
    has_before:
      direction === "older"
        ? page.has_before
        : current.has_before || removed || byteEviction,
    has_after:
      direction === "newer"
        ? page.has_after
        : current.has_after || removed || byteEviction,
  };
}

// Snapshot tails have their own global budget, independent of the sliding history cache.
export function boundSnapshot(
  snapshot: Snapshot,
  preferredId?: string,
): Snapshot {
  let remaining = 1000;
  let available = 4 * 1024 * 1024;
  const tails = new Map<string, Message[]>();
  const conversations = [
    ...snapshot.rooms.map((room) => ({ id: room.id, messages: room.messages })),
    ...Object.values(snapshot.private_access).map((access) => ({
      id: access.id,
      messages: snapshot.direct.filter(
        (message) => message.private_id === access.id,
      ),
    })),
  ];
  conversations.sort(
    (a, b) => Number(b.id === preferredId) - Number(a.id === preferredId),
  );
  for (const conversation of conversations) {
    const messages: Message[] = [];
    for (const message of [...conversation.messages].reverse()) {
      const size = bytes([message]);
      if (remaining <= 0 || size > available) break;
      remaining--;
      available -= size;
      messages.unshift(message);
    }
    tails.set(conversation.id, messages);
  }
  return {
    ...snapshot,
    rooms: snapshot.rooms.map((room) => {
      const messages = tails.get(room.id) ?? [];
      return messages.length === room.messages.length
        ? room
        : { ...room, messages };
    }),
    direct: [...tails.entries()]
      .filter(([id]) => !snapshot.rooms.some((room) => room.id === id))
      .flatMap(([, messages]) => messages)
      .sort((a, b) => a.sequence - b.sequence),
  };
}
export function applyConversation(
  previous: Snapshot,
  update: {
    view: string;
    id: string;
    messages: Message[];
    unread: Snapshot["unread"][string];
  },
): Snapshot | null {
  const room = previous.rooms.find(
    (room) => room.name === update.view && room.id === update.id,
  );
  const direct = previous.private_access[update.view]?.id === update.id;
  if (!room && !direct) return null;
  return boundSnapshot(
    {
      ...previous,
      rooms: room
        ? previous.rooms.map((item) =>
            item.id === update.id
              ? { ...item, messages: update.messages }
              : item,
          )
        : previous.rooms,
      direct: direct
        ? [
            ...previous.direct.filter(
              (message) => message.private_id !== update.id,
            ),
            ...update.messages,
          ].sort((a, b) => a.sequence - b.sequence)
        : previous.direct,
      unread: { ...previous.unread, [update.view]: update.unread },
    },
    update.id,
  );
}
