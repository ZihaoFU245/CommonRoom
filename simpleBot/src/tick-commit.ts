import { idempotencyKey } from "./api.ts";
import type { AgentState } from "./agent-state.ts";
import type { Outbox } from "./outbox.ts";
import { ModelError, type CompletionRequest } from "./model.ts";
import { tickDecision, type TickDecision, type TickSession } from "./tick.ts";
import type { TaskRecord } from "./store.ts";
import type { Conversation, Message } from "./wire.ts";

/** Validate a whole plan and atomically publish its messages, jobs and continuation. */
export async function commitTick(
  host: AgentState,
  outbox: Outbox,
  conversation: Conversation,
  session: TickSession,
  root: TaskRecord,
  accepted: { task: TaskRecord; message: Message }[],
  request: CompletionRequest,
  decision: TickDecision,
  observed: string[],
): Promise<{ decision: TickDecision; jobIds: Map<string, string> }> {
  const { store, config } = host.options;
  const id = conversation.id;
  const tick = session.tick;
  const tools = (request.tools ?? []).filter(
    (tool) => !["finish_turn", "wait_jobs"].includes(tool.function.name),
  );
  try {
    decision = tickDecision(decision, tools);
  } catch {
    throw new ModelError(422, "Invalid tick decision");
  }
  const jobIds = new Map(
    decision.actions.map((action) => [
      action.id,
      idempotencyKey(id, root.input, `tick:${session.tick}:${action.id}`),
    ]),
  );
  if (
    session.trace.reduce((count, entry) => count + entry.actions.length, 0) +
      decision.actions.length >
      config.get("model.maxToolCalls") ||
    Object.keys(session.jobs).length + decision.actions.length > 128
  )
    throw new ModelError(422, "Episode tool budget exhausted");
  if (decision.next.kind === "on_jobs") {
    decision.next.jobs = decision.next.jobs.map(
      (reference) => jobIds.get(reference) ?? reference,
    );
    if (
      decision.next.jobs.some(
        (reference) =>
          !(reference in session.jobs) &&
          ![...jobIds.values()].includes(reference),
      )
    )
      throw new ModelError(422, "Unknown continuation job");
  }
  const sources = new Set([
    ...session.sources.map((item) => item.input),
    ...accepted.map((item) => item.task.input),
  ]);
  for (const message of decision.messages)
    if (
      message.replyTo !== undefined &&
      (!conversation.permissions.reply || !sources.has(message.replyTo))
    )
      throw new ModelError(422, "Reply target must be an admitted input");
  if (sources.size > 400) throw new Error("Episode input bound exhausted");
  // This synchronous transition is snapshotted together by Store.save().
  for (const item of accepted) {
    const known = session.sources.find(
      (source) => source.input === item.task.input,
    );
    if (known) known.hash = item.task.promptHash;
    else
      session.sources.push({
        input: item.task.input,
        hash: item.task.promptHash,
        sequence: item.task.sequence,
      });
    item.task.status = "answered";
    item.task.updated = Date.now();
  }
  root.status = "answered";
  const slot = !session.trace.length
    ? (root.outputSlot ?? "answer")
    : `tick:${tick}`;
  root.outputSlot = slot;
  root.outputHash = root.promptHash;
  root.decision = decision.messages.map((message) => ({
    text: message.text,
    ...(message.replyTo ? { reply_to: message.replyTo } : {}),
  }));
  for (const [index, message] of decision.messages.entries())
    outbox.stageParts(
      conversation,
      root,
      decision.messages.length === 1 ? slot : `${slot}:${index}`,
      message.text,
      message.replyTo,
    );
  if (!decision.messages.length) root.silentHash = root.promptHash;
  const created = Date.now();
  for (const action of decision.actions) {
    const jobId = jobIds.get(action.id)!;
    session.jobs[jobId] = {
      id: jobId,
      root: root.input,
      sources: [...sources],
      tool: action.tool,
      arguments: JSON.stringify(action.arguments),
      status: "queued",
      created,
    };
  }
  for (const jobId of observed)
    if (session.jobs[jobId]) session.jobs[jobId].observed = true;
  session.next = decision.next;
  if (decision.next.kind === "after")
    session.dueAt = created + decision.next.delayMs;
  session.phase = "idle";
  session.trace.push({
    tick,
    messages: decision.messages.map((message) => message.text.slice(0, 4000)),
    actions: [...jobIds.values()],
  });
  if (session.trace.length > 32) session.trace.shift();
  delete session.error;
  session.retries = 0;
  delete session.retryInputs;
  await store.save();
  return { decision, jobIds };
}
