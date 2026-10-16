import { idempotencyKey } from "./api.ts";
import type { AgentState } from "./agent-state.ts";
import type { Outbox } from "./outbox.ts";
import { ModelError, type CompletionRequest } from "./model.ts";
import {
  jobTerminal,
  tickDecision,
  type TickDecision,
  type TickSession,
} from "./tick.ts";
import type { TaskRecord } from "./store.ts";
import type { Conversation, Message } from "./wire.ts";
import { acceptReplies } from "./tick-replies.ts";

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
  } catch (error) {
    throw new ModelError(
      422,
      `Invalid tick decision: ${error instanceof Error ? error.message : "invalid arguments"}`,
    );
  }
  const spent = session.trace.reduce(
    (count, entry) => count + entry.actions.length,
    0,
  );
  const allowance = Math.max(
    0,
    Math.min(
      config.get("model.maxToolCalls") - spent,
      128 - Object.keys(session.jobs).length,
    ),
  );
  let droppedActions = 0;
  if (decision.actions.length > allowance) {
    // A host budget is not a model mistake. Keep the messages the model wrote,
    // stop the episode, and never leave a continuation that references an action
    // this commit dropped.
    droppedActions = decision.actions.length - allowance;
    decision.actions = decision.actions.slice(0, allowance);
    decision.next = { kind: "wait" };
  }
  const allowedActions = decision.actions.length;
  const jobIds = new Map(
    decision.actions.map((action) => [
      action.id,
      idempotencyKey(id, root.input, `tick:${session.tick}:${action.id}`),
    ]),
  );
  const replies = acceptReplies(
    host,
    conversation,
    session,
    accepted,
    decision,
  );
  // Provider schemas stay static. Apply the live limit after combining parts
  // for the same input and dropping replies that were already committed.
  if (replies.messages.length > (request.maxReplyMessages ?? 16))
    throw new ModelError(422, "Reply plan exceeds this tick's message limit");
  if (droppedActions && !decision.messages.length && !replies.finished) {
    // The host budget stopped this plan and the model produced no reply: commit
    // nothing and let the bounded tool-less recovery tick answer in prose.
    throw new Error("Episode tool budget exhausted before any reply");
  }
  if (decision.next.kind === "on_jobs") {
    decision.next.jobs = decision.next.jobs.map(
      (reference) => jobIds.get(reference) ?? reference,
    );
    if (
      decision.next.jobs.some(
        (reference) =>
          !Object.hasOwn(session.jobs, reference) &&
          ![...jobIds.values()].includes(reference),
      )
    )
      throw new ModelError(422, "Unknown continuation job");
    // Results delivered in this request are consumed by this commit. Re-watching
    // only those results must not create another event or inference call.
    if (
      decision.next.jobs.every((reference) => {
        const job = session.jobs[reference];
        return (
          job &&
          jobTerminal(job) &&
          (job.observed || observed.includes(reference))
        );
      })
    ) {
      if (!replies.finished)
        throw new ModelError(
          422,
          "next.jobs: all watched results were already delivered. Use the results now, choose continue for more work, or wait for intentional silence; do not watch observed jobs again",
        );
      host.options.log(
        "debug",
        `tick watch consumed conversation=${id} tick=${tick} watched=${JSON.stringify(decision.next.jobs)} next=wait`,
      );
      decision.next = { kind: "wait" };
    }
  }
  const sources = new Set([
    ...session.sources.map((item) => item.input),
    ...accepted.map((item) => item.task.input),
  ]);
  decision.messages = replies.messages;
  if (
    replies.finished &&
    decision.next.kind === "continue" &&
    !decision.actions.length &&
    !Object.values(session.jobs).some(
      (job) => job.status === "queued" || job.status === "running",
    )
  )
    decision.next = { kind: "wait" };
  if (sources.size > 400) {
    // Release the oldest bookkeeping instead of failing an admitted input
    // because this episode's source list grew past its bound.
    const keep = [...sources].slice(-400);
    sources.clear();
    for (const input of keep) sources.add(input);
    session.sources = session.sources.filter((item) => sources.has(item.input));
    host.options.log(
      "warn",
      `episode input bound reached conversation=${id} tick=${tick} tracked=${sources.size}; releasing oldest inputs`,
    );
  }
  // This synchronous transition is snapshotted together by Store.save().
  for (const [task, hash] of replies.replied) task.replyHash = hash;
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
  if (droppedActions)
    host.options.log(
      "warn",
      `episode tool budget reached conversation=${id} tick=${tick} allowed=${allowedActions} dropped=${droppedActions}; messages published and the episode stopped`,
    );
  host.options.log(
    "debug",
    `tick replies committed conversation=${id} tick=${tick} inputs=${JSON.stringify(replies.replied.map(([task]) => task.input))} targets=${JSON.stringify(decision.messages.map((message) => message.replyTo ?? null))}`,
  );
  return { decision, jobIds };
}
