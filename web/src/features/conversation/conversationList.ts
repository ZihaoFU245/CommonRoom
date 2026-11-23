import type { Message, Snapshot, Unread } from "../../api/protocol.ts";

interface ConversationEntry {
  view: string;
  kind: "room" | "direct";
  /** Displayed room name or pair, or the peer's username for direct views. */
  name: string;
}

const chatNames = new Intl.Collator(undefined, {
  sensitivity: "base",
  numeric: true,
});

/** Unread chats lead by latest activity. Other chats sort by displayed name. */
export function conversationList(
  rooms: readonly { name: string }[],
  peers: readonly string[],
  unread: Readonly<Record<string, Unread>>,
  displayNames?: Readonly<Record<string, string>>,
  markedUnread?: (view: string) => boolean,
): ConversationEntry[] {
  const entries: ConversationEntry[] = [
    ...rooms.map(({ name }) => ({
      view: name,
      kind: "room" as const,
      name: name.startsWith("@private:") ? name.slice(9) : name,
    })),
    ...peers.map((name) => ({
      view: `@direct:${name}`,
      kind: "direct" as const,
      name,
    })),
  ];
  const unreadActivity = (view: string) => {
    const status = unread[view];
    return status?.count || markedUnread?.(view)
      ? (status?.through ?? 0) + 1
      : 0;
  };
  const label = ({ kind, name }: ConversationEntry) =>
    kind === "direct" && displayNames && Object.hasOwn(displayNames, name)
      ? (displayNames[name] ?? name)
      : name;
  return entries.sort(
    (a, b) =>
      unreadActivity(b.view) - unreadActivity(a.view) ||
      chatNames.compare(label(a), label(b)) ||
      a.view.localeCompare(b.view),
  );
}

interface ChatActivity {
  latest: Message;
  unread?: Message;
}

/** Index the current tails once, including private views that share one tail. */
export function conversationActivity(
  state: Pick<
    Snapshot,
    "rooms" | "direct" | "private_access" | "unread" | "username"
  >,
) {
  const activity = new Map<string, ChatActivity>();
  function add(view: string, message: Message) {
    const unread = state.unread[view];
    if (
      unread &&
      (message.sequence < unread.oldest || message.sequence > unread.through)
    )
      return;
    const current = activity.get(view);
    const latest =
      !current || message.sequence > current.latest.sequence
        ? message
        : current.latest;
    const newestUnread =
      unread?.count &&
      unread.first !== null &&
      message.from !== state.username &&
      message.sequence >= unread.first &&
      (!current?.unread || message.sequence > current.unread.sequence)
        ? message
        : current?.unread;
    activity.set(view, {
      latest,
      ...(newestUnread && { unread: newestUnread }),
    });
  }
  for (const room of state.rooms) {
    for (const message of room.messages) add(room.name, message);
  }
  for (const message of state.direct) {
    const peer =
      message.from === state.username
        ? message.to
        : message.to === state.username
          ? message.from
          : null;
    if (!peer) continue;
    const view = `@direct:${peer}`;
    const currentId = state.private_access[view]?.id;
    if (currentId && message.private_id && message.private_id !== currentId)
      continue;
    add(view, message);
  }
  return activity;
}

export function conversationPreview(message: Message): string {
  // Keep the sidebar independent of the lazy Markdown renderer.
  const text = message.markdown
    ? message.text
        .replace(
          /(!?)\[([^\]\n]{0,256})\]\((?:<[^>\n]{1,512}>|[^\s)\n]{1,512})\)/gu,
          (_, image: string, label: string) =>
            label || (image ? "Image" : "Link"),
        )
        .replace(/(\*\*|__|~~|`)([^\n]{1,160}?)\1/gu, "$2")
    : message.text;
  const compact = text.trim().replace(/\s+/gu, " ");
  const characters = Array.from(compact);
  return characters.length > 160
    ? `${characters.slice(0, 159).join("")}…`
    : compact;
}
