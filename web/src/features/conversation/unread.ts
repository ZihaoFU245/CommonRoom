import type { Message, Unread } from "../../api/protocol.ts";
// A selected conversation keeps only its retained history. Snapshot tails
// replace cached copies so reactions/replies stay current.
export function retainedHistory(
  history: Message[],
  tail: Message[],
  metadata: Pick<Unread, "oldest" | "through">,
): Message[] {
  const messages = new Map<string, Message>();
  for (const message of [...history, ...tail]) {
    if (
      message.sequence >= metadata.oldest &&
      message.sequence <= metadata.through
    )
      messages.set(message.id, message);
  }
  return [...messages.values()].sort((a, b) => a.sequence - b.sequence);
}
export function visibleReadPosition(
  elements: { sequence: number; top: number; bottom: number }[],
  viewport: { top: number; bottom: number },
  firstUnread: number | null,
) {
  let through = 0;
  let firstLoaded = !firstUnread;
  for (const { sequence, top, bottom } of elements) {
    if (sequence === firstUnread) firstLoaded = true;
    // A partially clipped message has not been fully viewed yet.
    if (
      bottom <= viewport.bottom + 1 &&
      bottom > viewport.top &&
      top < viewport.bottom
    )
      through = Math.max(through, sequence);
  }
  return firstLoaded && (!firstUnread || through >= firstUnread) ? through : 0;
}
export function badgeLabel(count: number) {
  return count > 99 ? "99+" : String(count);
}
