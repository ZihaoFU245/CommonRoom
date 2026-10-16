import { chunkMessage, idempotencyKey } from "./api.ts";
import { taskKey, type TaskRecord } from "./store.ts";
import type { Conversation } from "./wire.ts";
import type { AgentState } from "./agent-state.ts";

export class Outbox {
  private readonly host: AgentState;
  constructor(host: AgentState) {
    this.host = host;
  }
  async sendParts(
    conversation: Conversation,
    task: TaskRecord,
    slot: string,
    text: string,
  ): Promise<void> {
    this.stageParts(conversation, task, slot, text, task.replyTo ?? undefined);
    await this.host.options.store.save();
    await this.deliverParts(conversation, task, slot);
  }

  stageParts(
    conversation: Conversation,
    task: TaskRecord,
    slot: string,
    text: string,
    replyTo?: string,
  ): void {
    const { store } = this.host.options;
    const parts = chunkMessage(text);
    const numbered = parts.length > 1;
    const key = taskKey(conversation.id, task.input);
    for (const [index, body] of parts.entries()) {
      const name = numbered ? `${slot}:${index + 1}` : slot;
      const full = `${key}:${name}`;
      const labelled = numbered
        ? `[${index + 1}/${parts.length}] ${body}`
        : body;
      let part = store.part(full);
      if (part === undefined) {
        part = {
          slot: full,
          key: idempotencyKey(conversation.id, task.input, name),
          request: {
            conversation_id: conversation.id,
            text: labelled,
            ...(replyTo === undefined ? {} : { reply_to: replyTo }),
            markdown: true,
          },
          created: Date.now(),
        };
        store.putPart(part);
        if (!task.slots.includes(name)) task.slots.push(name);
        store.putTask(task);
      }
    }
  }

  async deliverParts(
    conversation: Conversation,
    task: TaskRecord,
    slot: string,
    signal?: AbortSignal,
  ): Promise<boolean> {
    const { store } = this.host.options;
    const key = taskKey(conversation.id, task.input);
    const parts = task.slots
      .filter((name) => !slot || name === slot || name.startsWith(`${slot}:`))
      .map((name) => store.part(`${key}:${name}`));
    if (parts.length === 0) return false;
    for (const part of parts) {
      signal?.throwIfAborted();
      if (this.host.stopped)
        throw new Error("worker stopped for transport change");
      if (part === undefined)
        throw new Error("checkpoint output batch is incomplete");
      if (task.status === "cancelled") return true;
      if (part.response !== undefined) continue;
      if (Date.now() - part.created >= 23 * 60 * 60 * 1000) {
        throw new Error(
          "pending send is near receipt expiry; inspect history before resolving it",
        );
      }
      this.host.metrics(conversation.id).deliveryStarted(part.slot);
      const response = await this.host.options.api.send(part.request, part.key);
      part.response = {
        message: {
          id: response.message.id,
          sequence: response.message.sequence,
        },
        replayed: response.replayed,
      };
      store.putPart(part);
      if (response.replayed) store.metrics.replayed++;
      else store.metrics.sent++;
      await store.save();
      this.host.metrics(conversation.id).delivered(part.slot);
      this.host.options.log(
        "debug",
        `reply delivered conversation=${conversation.id} input=${task.input} slot=${part.slot} reply_to=${part.request.reply_to ?? "none"} message=${response.message.id} replayed=${response.replayed}`,
      );
    }
    return true;
  }
}
