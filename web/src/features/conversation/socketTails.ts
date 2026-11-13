import type {
  ConversationDelta,
  ConversationUpdate,
  Message,
  Snapshot,
} from "../../api/protocol.ts";

// The wire window is independent of history paging and UI message budgets.
export class SocketTails {
  private tails = new Map<string, { revision: number; messages: Message[] }>();

  constructor(snapshot?: Snapshot) {
    if (!snapshot) return;
    for (const room of snapshot.rooms) {
      const unread = snapshot.unread[room.name];
      if (unread)
        this.tails.set(room.id, {
          revision: unread.revision,
          messages: room.messages,
        });
    }
    for (const [view, access] of Object.entries(snapshot.private_access)) {
      const unread = snapshot.unread[view];
      if (unread)
        this.tails.set(access.id, { revision: unread.revision, messages: [] });
    }
    for (const message of snapshot.direct) {
      if (message.private_id)
        this.tails.get(message.private_id)?.messages.push(message);
    }
  }

  apply(
    update: ConversationUpdate | ConversationDelta,
  ): ConversationUpdate | null {
    if (update.kind === "conversation") {
      this.tails.set(update.id, {
        revision: update.unread.revision,
        messages: update.messages,
      });
      return update;
    }
    const tail = this.tails.get(update.id);
    if (!tail || tail.revision !== update.base_revision) return null;
    const bySequence = new Map(
      tail.messages.map((message) => [message.sequence, message]),
    );
    for (const message of update.upsert)
      bySequence.set(message.sequence, message);
    const messages: Message[] = [];
    for (const sequence of update.sequences) {
      const message = bySequence.get(sequence);
      if (!message) return null;
      messages.push(message);
    }
    const next: ConversationUpdate = {
      ...update,
      kind: "conversation",
      messages,
    };
    this.tails.set(update.id, { revision: update.unread.revision, messages });
    return next;
  }
}
