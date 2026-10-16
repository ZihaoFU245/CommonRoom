import type { AgentState } from "./agent-state.ts";
import type { Outbox } from "./outbox.ts";
import { taskKey } from "./store.ts";

/** V1 saved group decisions are committed plans, not new model work. */
export async function recoverLegacyGroups(
  host: AgentState,
  outbox: Outbox,
): Promise<void> {
  const store = host.options.store;
  for (const task of store.tasks()) {
    if (!task.batch || task.status === "cancelled") continue;
    const conversation = host.snapshot?.conversations.find(
      (item) => item.id === task.conversation,
    );
    if (!conversation || !conversation.permissions.send) continue;
    const members = task.batch.map((input) => ({
      input,
      task: store.task(task.conversation, input.input),
    }));
    const session = store.state.sessions[task.conversation];
    if (
      task.decision === undefined ||
      (session?.root === task.input && session.trace.length)
    ) {
      // Incomplete old inference must release its batch followers. A v2 tick
      // trace already owns its output; stale v1 batch metadata must not stage
      // that output again under a different family of idempotency keys.
      for (const { task: member } of members)
        if (member) delete member.batchOwner;
      delete task.batch;
      await store.save();
      continue;
    }
    const changed = members.some(
      ({ input, task: member }) =>
        !member ||
        member.status === "cancelled" ||
        member.promptHash !== input.hash,
    );
    if (changed) {
      const key = taskKey(task.conversation, task.input);
      const unsent = task.slots.filter(
        (slot) => !store.part(`${key}:${slot}`)?.response,
      );
      store.deleteParts(unsent.map((slot) => `${key}:${slot}`));
      task.slots = task.slots.filter((slot) => !unsent.includes(slot));
      delete task.decision;
    } else {
      const slot = task.outputSlot ?? "group:0";
      for (const [index, response] of task.decision.entries())
        outbox.stageParts(
          conversation,
          task,
          `${slot}:${index}`,
          response.text,
          response.reply_to,
        );
      for (const { input, task: member } of members) {
        if (!member) continue;
        member.status = "answered";
        if (task.decision.length) member.replyHash = input.hash;
        else member.silentHash = input.hash;
      }
      task.outputHash = task.promptHash;
    }
    for (const { task: member } of members)
      if (member) delete member.batchOwner;
    delete task.batch;
    await store.save();
  }
}
