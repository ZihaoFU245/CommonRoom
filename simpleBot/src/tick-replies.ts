import type { AgentState } from "./agent-state.ts";
import { ModelError } from "./model.ts";
import type { TaskRecord } from "./store.ts";
import type { TickDecision, TickSession } from "./tick.ts";
import type { Conversation, Message } from "./wire.ts";

/** Consumption and final response ownership are separate durable facts. */
export function acceptReplies(
  host: AgentState,
  conversation: Conversation,
  session: TickSession,
  accepted: { task: TaskRecord; message: Message }[],
  decision: TickDecision,
): {
  messages: TickDecision["messages"];
  replied: [TaskRecord, string][];
  finished: boolean;
} {
  const sources = new Map(
    session.sources.map((source) => [source.input, source.hash]),
  );
  for (const { task } of accepted) sources.set(task.input, task.promptHash);
  const eligible = new Map<string, [TaskRecord, string]>();
  for (const [id, hash] of sources) {
    const task = host.options.store.task(conversation.id, id);
    if (task && task.replyHash !== hash) eligible.set(id, [task, hash]);
  }
  const replied = new Map<string, [TaskRecord, string]>();
  const messages: TickDecision["messages"] = [];
  for (const message of decision.messages) {
    if (message.replyTo !== undefined) {
      if (!conversation.permissions.reply || !sources.has(message.replyTo))
        throw new ModelError(
          422,
          "Reply target must be an admitted input; use a replyTargets ID from Tick state or omit replyTo",
        );
      const source = eligible.get(message.replyTo);
      if (!source) continue;
      replied.set(message.replyTo, source);
      messages.push(message);
    } else if (eligible.size) {
      // An untargeted combined answer consumes all outstanding inputs. Keep
      // native reply threading for DMs and addressed, non-autonomous messages.
      const target =
        accepted.find(({ task }) => eligible.has(task.input))?.task ??
        [...eligible.values()].at(-1)?.[0];
      messages.push({
        ...message,
        ...(target?.replyTo &&
        !target.autonomous &&
        conversation.permissions.reply
          ? { replyTo: target.replyTo }
          : {}),
      });
      for (const [id, source] of eligible) replied.set(id, source);
    }
  }
  const combined: TickDecision["messages"] = [];
  const byInput = new Map<string, number>();
  // A plain group response covers the outstanding batch. Combining all parts of
  // that plan also prevents a plain response plus a targeted reply from
  // answering the same batch member twice. Targeted-only plans stay independent.
  const plain = messages.find((message) => message.replyTo === undefined);
  for (const message of messages) {
    const target = plain
      ? "combined-batch"
      : (message.replyTo ??
        (eligible.size === 1 ? eligible.keys().next().value : undefined));
    const index = target === undefined ? undefined : byInput.get(target);
    if (index !== undefined) {
      if (combined[index].text.trim() !== message.text.trim())
        combined[index].text += `\n\n${message.text}`;
      if (Array.from(combined[index].text).length > 262144)
        throw new ModelError(
          422,
          "Combined input response exceeds the output limit",
        );
    } else {
      if (target !== undefined) byInput.set(target, combined.length);
      combined.push(plain ? { text: message.text } : { ...message });
    }
  }
  return {
    messages: combined,
    replied: [...replied.values()],
    finished: eligible.size === replied.size,
  };
}
