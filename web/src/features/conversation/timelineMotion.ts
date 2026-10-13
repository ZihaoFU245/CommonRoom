import type { TimelineEntry } from "../../api/protocol.ts";

export const messageMotion = { duration: 180, easing: "ease-out" };

/** Only live additions enter; loading an older page must not replay messages. */
export function timelineChanges(
  before: TimelineEntry[],
  after: TimelineEntry[],
) {
  const previous = new Map(before.map((entry) => [entry.key, entry]));
  const current = new Set(after.map((entry) => entry.key));
  const latest = before.reduce(
    (sequence, entry) =>
      entry.kind === "message"
        ? Math.max(sequence, entry.message.sequence)
        : sequence,
    0,
  );
  const added = new Set<string>();
  const edited = new Set<string>();
  for (const entry of after) {
    if (entry.kind !== "message") continue;
    const old = previous.get(entry.key);
    if (!old && entry.message.sequence > latest) added.add(entry.key);
    if (
      old?.kind === "message" &&
      (old.message.text !== entry.message.text ||
        (old.message.markdown ?? false) !== (entry.message.markdown ?? false))
    )
      edited.add(entry.key);
  }
  const removed = new Set(
    before
      .filter((entry) => entry.kind === "message" && !current.has(entry.key))
      .map((entry) => entry.key),
  );
  return { added, edited, removed };
}
