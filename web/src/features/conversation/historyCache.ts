import { mergeMessageColors } from "../preferences/nameColors.ts";
import type {
  History,
  Message,
  Snapshot,
  Unread,
  MessageColors,
} from "../../api/protocol.ts";
export const ROOM_CACHE_LIMIT = 1000;
const TOTAL_CACHE_LIMIT = 5000;
const BYTE_LIMIT = 16 * 1024 * 1024;
export interface CachedHistory extends History {
  id: string;
  through: number;
  oldest: number;
}
export function messageBytes(message: Message): number {
  let characters =
    message.text.length +
    message.id.length +
    message.from.length +
    (message.reply?.text.length ?? 0);
  for (const mention of message.mentions) characters += mention.length;
  for (const [reaction, users] of Object.entries(message.reactions)) {
    characters += reaction.length;
    for (const user of users) characters += user.length;
  }
  return 512 + 2 * characters;
}
export function bytes(messages: Message[]): number {
  return messages.reduce((sum, message) => sum + messageBytes(message), 0);
}

export class HistoryCache {
  private entries = new Map<string, { page: CachedHistory; bytes: number }>();
  private count = 0;
  private sizeBytes = 0;
  get size(): number {
    return this.entries.size;
  }
  get(view: string): CachedHistory | undefined {
    return this.entries.get(view)?.page;
  }
  has(view: string): boolean {
    return this.entries.has(view);
  }
  *values(): IterableIterator<CachedHistory> {
    for (const entry of this.entries.values()) yield entry.page;
  }
  *[Symbol.iterator](): IterableIterator<[string, CachedHistory]> {
    for (const [view, entry] of this.entries) yield [view, entry.page];
  }
  delete(view: string): void {
    const entry = this.entries.get(view);
    if (!entry) return;
    this.count -= entry.page.messages.length;
    this.sizeBytes -= entry.bytes;
    this.entries.delete(view);
  }
  store(page: CachedHistory): void {
    const previous = this.entries.get(page.view);
    const size =
      previous?.page.messages === page.messages &&
      previous.page.name_colors === page.name_colors
        ? previous.bytes
        : bytes(page.messages) +
          Object.entries(page.name_colors?.values ?? {}).reduce(
            (total, [name, color]) =>
              total + 128 + 2 * (name.length + (color?.length ?? 0)),
            0,
          );
    this.delete(page.view);
    this.entries.set(page.view, { page, bytes: size });
    this.count += page.messages.length;
    this.sizeBytes += size;
    while (
      this.count > TOTAL_CACHE_LIMIT ||
      this.sizeBytes > BYTE_LIMIT ||
      this.size > 32
    ) {
      const oldest = this.entries.keys().next().value;
      if (oldest === undefined) break;
      this.delete(oldest);
    }
  }
}
/** Keep cached windows visible while applying authoritative retention/tail facts. */
export function previewPage(
  page: CachedHistory,
  unread: Unread,
  tail: Message[],
): CachedHistory {
  if (page.revision === unread.revision) return page;
  const latest = new Map(tail.map((message) => [message.id, message]));
  const first = tail[0]?.sequence;
  const coversTail =
    first !== undefined && tail.at(-1)?.sequence === unread.through;
  return {
    ...page,
    messages: page.messages
      .filter(
        (message) =>
          message.sequence >= unread.oldest &&
          message.sequence <= unread.through &&
          !(
            coversTail &&
            first !== undefined &&
            message.sequence >= first &&
            !latest.has(message.id)
          ),
      )
      .map((message) => latest.get(message.id) ?? message),
  };
}

export function mergePage(
  current: CachedHistory,
  page: History,
  direction: "older" | "newer",
): CachedHistory {
  // Both inputs are ordered by stable sequence; the fetched page wins overlaps.
  const sorted: Message[] = [];
  let left = 0;
  let right = 0;
  while (left < current.messages.length || right < page.messages.length) {
    const old = current.messages[left];
    const fresh = page.messages[right];
    if (old && (!fresh || old.sequence < fresh.sequence)) {
      sorted.push(old);
      left++;
    } else if (fresh) {
      sorted.push(fresh);
      right++;
      if (old?.sequence === fresh.sequence) left++;
    }
  }
  const removed = sorted.length > ROOM_CACHE_LIMIT;
  let start =
    direction === "older" ? 0 : Math.max(0, sorted.length - ROOM_CACHE_LIMIT);
  let end =
    direction === "older"
      ? Math.min(sorted.length, ROOM_CACHE_LIMIT)
      : sorted.length;
  const sizes = sorted.map(messageBytes);
  let size = 0;
  for (let i = start; i < end; i++) size += sizes[i] ?? 0;
  let byteEviction = false;
  while (size > 4 * 1024 * 1024 && end - start > 1) {
    size -= direction === "older" ? (sizes[--end] ?? 0) : (sizes[start++] ?? 0);
    byteEviction = true;
  }
  const messages = sorted.slice(start, end);
  return {
    ...current,
    messages,
    name_colors: mergeMessageColors(
      current.name_colors,
      page.name_colors,
      messages,
    ),
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
  const direct = new Map<string, Message[]>();
  for (const message of snapshot.direct) {
    if (!message.private_id) continue;
    const messages = direct.get(message.private_id);
    if (messages) messages.push(message);
    else direct.set(message.private_id, [message]);
  }
  const roomIds = new Set(snapshot.rooms.map((room) => room.id));
  const conversations = [
    ...snapshot.rooms.map((room) => ({ id: room.id, messages: room.messages })),
    ...Object.values(snapshot.private_access).map((access) => ({
      id: access.id,
      messages: direct.get(access.id) ?? [],
    })),
  ];
  conversations.sort(
    (a, b) => Number(b.id === preferredId) - Number(a.id === preferredId),
  );
  for (const conversation of conversations) {
    const messages: Message[] = [];
    for (let index = conversation.messages.length - 1; index >= 0; index--) {
      const message = conversation.messages[index];
      if (!message) continue;
      const size = messageBytes(message);
      if (remaining <= 0 || size > available) break;
      remaining--;
      available -= size;
      messages.push(message);
    }
    messages.reverse();
    tails.set(conversation.id, messages);
  }
  const directMessages = [...tails.entries()]
    .filter(([id]) => !roomIds.has(id))
    .flatMap(([, messages]) => messages)
    .sort((a, b) => a.sequence - b.sequence);
  return {
    ...snapshot,
    name_colors: mergeMessageColors(
      undefined,
      snapshot.name_colors,
      directMessages,
    ),
    rooms: snapshot.rooms.map((room) => {
      const messages = tails.get(room.id) ?? [];
      return messages.length === room.messages.length
        ? room
        : {
            ...room,
            messages,
            name_colors: mergeMessageColors(
              undefined,
              room.name_colors,
              messages,
            ),
          };
    }),
    direct: directMessages,
  };
}
export function applyConversation(
  previous: Snapshot,
  update: {
    view: string;
    id: string;
    messages: Message[];
    unread: Snapshot["unread"][string];
    name_colors?: MessageColors | undefined;
  },
): Snapshot | null {
  const room = previous.rooms.find(
    (room) => room.name === update.view && room.id === update.id,
  );
  const direct = previous.private_access[update.view]?.id === update.id;
  if (!room && !direct) return null;
  if (
    update.name_colors &&
    update.name_colors.version !== previous.color_version
  )
    return null;
  return boundSnapshot(
    {
      ...previous,
      name_colors: direct
        ? mergeMessageColors(previous.name_colors, update.name_colors, [
            ...previous.direct,
            ...update.messages,
          ])
        : previous.name_colors,
      rooms: room
        ? previous.rooms.map((item) =>
            item.id === update.id
              ? {
                  ...item,
                  messages: update.messages,
                  name_colors: update.name_colors,
                }
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
