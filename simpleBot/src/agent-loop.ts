import { commitTick } from "./tick-commit.ts";
import { appendTickState, requestTick } from "./tick-request.ts";
import type { AgentState } from "./agent-state.ts";
import { AgentJobs } from "./agent-jobs.ts";
import type { ModelContext } from "./agent-model.ts";
import type { Outbox } from "./outbox.ts";
import { Scheduler } from "./scheduler.ts";
import { abortable } from "./async.ts";
import { ModelError } from "./model.ts";
import { jobTerminal, sessionReady, type TickSession } from "./tick.ts";
import { taskKey, textHash, type TaskRecord } from "./store.ts";
import type { Message } from "./wire.ts";
import { groupInputs, GROUP_INTERVAL_MS } from "./group.ts";

function repairing(session: TickSession | undefined): boolean {
  return (
    !!session?.error &&
    !!session.retryInputs?.length &&
    session.phase !== "failed"
  );
}

/** Conversation scheduling, one-call ticks and durable continuation decisions. */
export class AgentLoop {
  private readonly host: AgentState;
  private readonly context: ModelContext;
  private readonly outbox: Outbox;
  private readonly scheduler: Scheduler;
  private readonly delivery: Scheduler;
  private readonly jobs: AgentJobs;
  private readonly progress: () => void;
  private readonly activeInputs = new Map<string, string[]>();
  private readonly controllers = new Map<string, AbortController>();

  constructor(
    host: AgentState,
    context: ModelContext,
    outbox: Outbox,
    progress: () => void,
  ) {
    this.host = host;
    this.progress = progress;
    this.context = context;
    this.outbox = outbox;
    const report = (error: unknown) =>
      host.options.log(
        "error",
        `agent event failed: ${error instanceof Error ? error.message : "unknown failure"}`,
      );
    this.scheduler = new Scheduler(
      (id) => this.tick(id),
      () => host.options.config.get("agent.maxConcurrentTicks"),
      report,
    );
    this.delivery = new Scheduler(
      (id) => this.flush(id),
      () => 64,
      report,
    );
    this.jobs = new AgentJobs(host, (id) => this.wake(id));
  }

  get busy(): boolean {
    return this.scheduler.busy || this.delivery.busy;
  }

  private queued(id: string): TaskRecord[] {
    return this.host.options.store
      .tasks()
      .filter(
        (task) =>
          task.conversation === id &&
          task.status === "queued" &&
          !task.batchOwner,
      )
      .sort(
        (a, b) =>
          Number(!!a.autonomous) - Number(!!b.autonomous) ||
          a.sequence - b.sequence,
      );
  }

  wake(id: string): void {
    if (this.host.stopped) return;
    const conversation = this.host.snapshot?.conversations.find(
      (item) => item.id === id,
    );
    if (!conversation) {
      this.invalidate(id);
      return;
    }
    const store = this.host.options.store;
    const session = store.state.sessions[id];
    const queued = this.queued(id);
    const now = Date.now();
    const deadlines = (repairing(session) ? [] : queued).map((task) =>
      task.autonomous ? (task.readyAt ?? now) : now,
    );
    if (session && session.next.kind !== "wait" && session.phase !== "failed") {
      deadlines.push(
        session.started +
          this.host.options.config.get("agent.episodeTimeoutMs"),
      );
      if (sessionReady(session, now)) deadlines.push(now);
      else if (session.next.kind === "after")
        deadlines.push(session.dueAt ?? now);
    }
    this.scheduler.cancelTimer(id);
    if (deadlines.length) {
      const deadline = Math.min(...deadlines);
      if (deadline <= now) this.scheduler.wake(id);
      else this.scheduler.at(id, deadline);
    }
    if (
      store
        .tasks()
        .some(
          (task) =>
            task.conversation === id &&
            task.status !== "cancelled" &&
            task.slots.some(
              (slot) =>
                store.part(`${taskKey(id, task.input)}:${slot}`)?.response ===
                undefined,
            ),
        )
    )
      this.delivery.wake(id);
  }

  invalidate(id: string, source?: string): void {
    const { store } = this.host.options;
    const existing = store.state.sessions[id];
    const active = this.activeInputs.get(id) ?? [];
    if (
      source &&
      existing &&
      source !== existing.root &&
      !existing.sources.some((item) => item.input === source) &&
      !active.includes(source) &&
      !Object.values(existing.jobs).some(
        (job) => job.sources?.includes(source) && !jobTerminal(job),
      )
    )
      return;
    if (existing) {
      const root = store.task(id, existing.root);
      const unsent =
        root?.slots.filter(
          (slot) =>
            store.part(`${taskKey(id, root.input)}:${slot}`)?.response ===
            undefined,
        ) ?? [];
      if (root && unsent.length) {
        store.deleteParts(
          unsent.map((slot) => `${taskKey(id, root.input)}:${slot}`),
        );
        root.slots = root.slots.filter((slot) => !unsent.includes(slot));
        delete root.outputHash;
        delete root.silentHash;
        root.pass = (root.pass ?? 0) + 1;
        root.outputSlot = `edit:${root.pass}`;
      }
      const requeue = active.length
        ? active
        : unsent.length
          ? existing.sources.map((item) => item.input)
          : [];
      for (const input of requeue) {
        const task = store.task(id, input);
        if (task && task.status !== "cancelled") task.status = "queued";
      }
    }
    this.controllers.get(id)?.abort();
    this.controllers.delete(id);
    this.jobs.cancel(id);
    this.scheduler.cancelTimer(id);
    const session = this.host.options.store.state.sessions[id];
    if (session) {
      for (const job of Object.values(session.jobs))
        if (job.status === "queued") {
          job.status = "interrupted";
          job.result = JSON.stringify({
            error: "Cancelled before job dispatch",
          });
        }
      session.generation++;
      session.phase = "idle";
      session.next = { kind: "wait" };
      delete session.dueAt;
    }
  }

  /** Compatibility for callers wanting completed currently admitted work. */
  async settle(): Promise<void> {
    for (;;) {
      await this.scheduler.settle();
      await this.delivery.settle();
      const waiting = this.host.options.store
        .tasks()
        .filter(
          (task) =>
            task.status === "queued" &&
            task.autonomous &&
            !task.batchOwner &&
            !repairing(
              this.host.options.store.state.sessions[task.conversation],
            ),
        );
      const deadline = Math.min(...waiting.map((task) => task.readyAt ?? 0));
      if (
        !waiting.length ||
        this.host.stopped ||
        deadline > Date.now() + GROUP_INTERVAL_MS + 10
      )
        return;
      if (deadline > Date.now())
        await abortable(
          new Promise<void>((resolve) =>
            setTimeout(resolve, deadline - Date.now()),
          ),
          this.host.abort.signal,
        );
      for (const task of waiting) this.wake(task.conversation);
    }
  }

  async close(): Promise<void> {
    for (const controller of this.controllers.values()) controller.abort();
    await this.scheduler.close();
    await this.jobs.close();
    await this.delivery.close();
  }

  private async locate(
    conversation: string,
    task: TaskRecord,
  ): Promise<Message | undefined> {
    const page = await this.host.options.api.history({
      conversation_id: conversation,
      around: task.sequence,
      limit: 1,
    });
    return page.messages.find((message) => message.id === task.input);
  }

  private async tick(id: string): Promise<void> {
    const { store, config, log } = this.host.options;
    const conversation = this.host.snapshot?.conversations.find(
      (item) => item.id === id,
    );
    if (!conversation || this.host.stopped || !conversation.permissions.send)
      return;
    // Local controls have already run during input admission.
    let pending = this.queued(id).filter(
      (task) => !task.autonomous || (task.readyAt ?? 0) <= Date.now(),
    );
    let session = store.state.sessions[id];
    // A repair owns its admitted inputs until commit/failure. New messages stay
    // queued instead of inheriting the old input's retry counter and error.
    if (repairing(session)) pending = [];
    if (!pending.length && (!session || !sessionReady(session))) {
      if (
        session &&
        session.next.kind !== "wait" &&
        Date.now() >= session.started + config.get("agent.episodeTimeoutMs")
      )
        await this.fail(id, session, "Episode deadline exhausted");
      this.wake(id);
      return;
    }
    if (pending.length) {
      const first = pending[0];
      pending = first.autonomous
        ? pending.filter((task) => task.autonomous).slice(0, 100)
        : [first];
      if (first.autonomous) {
        const allowed = new Set(
          groupInputs(pending, first).map((source) => source.input),
        );
        pending = pending.filter((task) => allowed.has(task.input));
      }
    }
    if (
      pending.length === 1 &&
      ((pending[0].slots.length &&
        pending[0].outputHash === pending[0].promptHash) ||
        pending[0].silentHash === pending[0].promptHash)
    ) {
      pending[0].status = "answered";
      await store.save();
      this.delivery.wake(id);
      this.wake(id);
      return;
    }
    if (
      !session ||
      (pending.length &&
        (session.phase === "failed" || session.next.kind === "wait"))
    ) {
      const previous = session;
      const root = pending[0];
      if (!root) return;
      store.state.sessions[id] = {
        root: root.input,
        generation: previous?.generation ?? 0,
        tick: previous?.tick ?? 0,
        calls: 0,
        started: Date.now(),
        phase: "ready",
        next: { kind: "continue" },
        jobs: previous?.jobs ?? {},
        sources: [],
        trace: [],
      };
      session = store.state.sessions[id];
      // Keep unobserved/uncertain outcomes. Old successful observations can retire.
      for (const [jobId, job] of Object.entries(session.jobs))
        if (job.observed && jobTerminal(job) && job.status === "completed")
          delete session.jobs[jobId];
    }
    if (!session) return;
    const root = store.task(id, session.root);
    if (!root || root.status === "cancelled") {
      this.invalidate(id);
      return;
    }
    if (
      session.calls >= config.get("agent.maxTicks") ||
      (session.tokens ?? 0) >= config.get("agent.maxEpisodeTokens") ||
      Date.now() >= session.started + config.get("agent.episodeTimeoutMs")
    ) {
      await this.fail(
        id,
        session,
        "Tick/token budget or episode deadline exhausted",
      );
      return;
    }
    let controller = this.controllers.get(id);
    if (!controller || controller.signal.aborted) {
      controller = new AbortController();
      this.controllers.set(id, controller);
    }
    const generation = session.generation;
    const episodeSignal = AbortSignal.any([
      controller.signal,
      this.host.abort.signal,
    ]);
    const timeout = Math.max(
      1,
      Math.min(
        config.get("model.timeoutMs"),
        session.started + config.get("agent.episodeTimeoutMs") - Date.now(),
      ),
    );
    const signal = AbortSignal.any([
      episodeSignal,
      AbortSignal.timeout(timeout),
    ]);
    const observed = Object.values(session.jobs)
      .filter((job) => jobTerminal(job) && !job.observed)
      .map((job) => job.id);
    const previousNext = session.next;
    const wakeReason = pending.length
      ? "messages"
      : session.retryInputs?.length
        ? "retry"
        : observed.length
          ? "job_results"
          : previousNext.kind;
    const watched =
      previousNext.kind === "on_jobs"
        ? previousNext.jobs.map((jobId) => ({
            id: jobId,
            status: session.jobs[jobId]?.status ?? "missing",
            observed: session.jobs[jobId]?.observed ?? false,
          }))
        : [];
    session.phase = "calling";
    session.next = { kind: "wait" }; // Claim the previous one-shot wake.
    delete session.dueAt;
    session.tick++;
    session.calls++;
    root.attempts++;
    root.status = "running";
    for (const task of pending) task.status = "running";
    const tick = session.tick;
    log(
      "debug",
      `tick wake conversation=${id} tick=${tick} reason=${wakeReason} observed_jobs=${JSON.stringify(observed)} watched=${JSON.stringify(watched)}`,
    );
    const tickController = new AbortController();
    let tickSucceeded = false;
    let accepted: { task: TaskRecord; message: Message }[] = [];
    const current = () =>
      !episodeSignal.aborted &&
      !this.host.stopped &&
      session.generation === generation &&
      root.status !== "cancelled" &&
      this.host.snapshot?.conversations.some(
        (item) => item.id === id && item.permissions.send,
      );
    const valid = () => !signal.aborted && current();
    try {
      await store.save();
      const inputs = pending.length
        ? pending
        : session.retryInputs?.length
          ? session.retryInputs
              .map((input) => store.task(id, input))
              .filter((task): task is TaskRecord => task !== undefined)
          : session.sources.length
            ? session.sources
                .map((source) => store.task(id, source.input))
                .filter(
                  (task): task is TaskRecord =>
                    task !== undefined && task.status !== "cancelled",
                )
                .sort((a, b) => a.sequence - b.sequence)
            : [root];
      this.activeInputs.set(
        id,
        inputs.map((task) => task.input),
      );
      for (const task of inputs) {
        const message = await abortable(this.locate(id, task), signal);
        if (!message) {
          // Never drop an admitted input without a trace: the message is gone
          // from retained history (retracted, expired or never scanned).
          log(
            "warn",
            `input unavailable conversation=${id} input=${task.input} sequence=${task.sequence}; cancelling task`,
          );
          task.status = "cancelled";
          continue;
        }
        task.promptHash = textHash(message.text);
        store.rememberVersion(
          id,
          message.id,
          task.promptHash,
          message.sequence,
        );
        accepted.push({ task, message });
      }
      if (!accepted.length) {
        log(
          "warn",
          `no admitted input could be located conversation=${id} tick=${tick}; releasing the episode`,
        );
        this.invalidate(id);
        await store.save();
        return;
      }
      const continuation = !pending.length && !session.retryInputs?.length;
      const source = continuation ? accepted[accepted.length - 1] : accepted[0];
      const group = source.task.autonomous
        ? accepted.map((item) => item.message)
        : undefined;
      const request = await abortable(
        this.context.prepare(
          conversation,
          source.message,
          root.pass ? "revised" : "new",
          root,
          episodeSignal,
          group,
          continuation ? accepted.map((item) => item.message) : undefined,
        ),
        signal,
      );
      const inputVersions = new Map(
        session.sources.map((source) => [
          source.input,
          { hash: source.hash, sequence: source.sequence },
        ]),
      );
      for (const { task } of accepted)
        inputVersions.set(task.input, {
          hash: task.promptHash,
          sequence: task.sequence,
        });
      const tickInputs = [...inputVersions].map(([input, version]) => ({
        id: input,
        sequence: version.sequence,
        replyCommitted: store.task(id, input)?.replyHash === version.hash,
      }));
      request.replyTargets = conversation.permissions.reply
        ? tickInputs
            .filter((input) => !input.replyCommitted)
            .map((input) => input.id)
        : [];
      request.maxReplyMessages = tickInputs.filter(
        (input) => !input.replyCommitted,
      ).length;
      appendTickState(
        request,
        session,
        config,
        pending.length
          ? "messages"
          : observed.length
            ? "job_results"
            : "continuation",
        tickInputs,
      );
      request.signal = signal;
      if (!source.task.autonomous) this.host.beginActivity(id);
      log(
        "debug",
        `task started conversation=${id} input=${root.input} attempt=${root.attempts}`,
      );
      log(
        "info",
        `tick started conversation=${id} tick=${tick} sources=${accepted.length}`,
      );
      let decision = await requestTick(
        this.host.options.model,
        request,
        root,
        source.task,
        !!group,
        signal,
        tickController.signal,
        () => tickSucceeded,
      );
      if (!valid()) return;
      const committed = await commitTick(
        this.host,
        this.outbox,
        conversation,
        session,
        root,
        accepted,
        request,
        decision,
        observed,
      );
      decision = committed.decision;
      const jobIds = committed.jobIds;
      tickSucceeded = true;
      // Once committed, an inference timeout cannot strand accepted job intents.
      if (!current()) {
        for (const jobId of jobIds.values()) {
          session.jobs[jobId].status = "interrupted";
          session.jobs[jobId].result = JSON.stringify({
            error: "Cancelled before job dispatch",
          });
        }
        await store.save();
        return;
      }
      for (const jobId of jobIds.values())
        this.jobs.start(
          id,
          session,
          session.jobs[jobId],
          request,
          episodeSignal,
        );
      if (decision.messages.length) {
        this.host.beginActivity(id);
        this.delivery.wake(id);
      }
      tickSucceeded = true;
      log(
        "info",
        `tick committed conversation=${id} tick=${tick} next=${session.next.kind} inputs=${accepted.map((item) => item.task.input).join(",")} messages=${decision.messages.length} jobs=${jobIds.size}`,
      );
    } catch (error) {
      if (
        this.host.stopped ||
        episodeSignal.aborted ||
        session.generation !== generation
      ) {
        for (const item of accepted)
          if (item.task.status === "running") item.task.status = "queued";
        if (root.status === "running") root.status = "queued";
        if (session.phase === "calling") {
          session.phase = "ready";
          session.next = { kind: "continue" };
        }
        await store.save();
      } else {
        const message = error instanceof Error ? error.message : "Tick failed";
        const retry =
          (error instanceof ModelError &&
            (error.status === 0 ||
              error.status === 408 ||
              error.status === 429 ||
              error.status >= 500 ||
              error.status === 422)) ||
          (this.host.options.model.tick !== undefined &&
            error instanceof Error &&
            error.name === "TimeoutError");
        // Terminal failures are handled by fail(), which spends one bounded
        // tool-less recovery tick when an admitted input still has no reply.
        if (
          retry &&
          (session.retries ?? 0) < 2 &&
          session.calls < config.get("agent.maxTicks")
        ) {
          session.error = message;
          session.retries = (session.retries ?? 0) + 1;
          session.next = {
            kind: "after",
            delayMs:
              500 * 2 ** (session.retries - 1) +
              Math.floor(Math.random() * 100),
          };
          session.dueAt = Date.now() + session.next.delayMs;
          session.phase = "idle";
          log(
            "warn",
            `tick retry scheduled conversation=${id} tick=${tick} retry=${session.retries}/2 delay_ms=${session.next.delayMs} reason=${error instanceof ModelError ? message : "tick transport failure"}`,
          );
          // Inference failed: leave input owned by this episode, not consumed.
          for (const task of pending)
            if (!session.sources.some((source) => source.input === task.input))
              session.sources.push({
                input: task.input,
                hash: task.promptHash,
                sequence: task.sequence,
              });
          session.retryInputs = (
            accepted.length ? accepted.map((item) => item.task) : pending
          ).map((task) => task.input);
          await store.save();
        } else await this.fail(id, session, message);
      }
    } finally {
      if (!tickSucceeded) tickController.abort();
      this.activeInputs.delete(id);
      log(
        "debug",
        `task finished conversation=${id} input=${root.input} status=${root.status}`,
      );
      this.host.endActivity(id);
      this.wake(id);
      this.progress();
    }
  }

  /**
   * End an episode. Before that, spend one bounded tool-less recovery tick when
   * an admitted input still has no reply: a rejected plan or a host budget stop
   * must not leave a question unanswered in silence. The recovery tick proposes
   * no actions and no continuation, so it can never repeat an effect.
   */
  private async fail(
    id: string,
    session: TickSession,
    error: string,
  ): Promise<void> {
    const { store, config, log } = this.host.options;
    const unanswered = this.unanswered(id, session);
    if (
      this.host.options.model.tick !== undefined &&
      (session.retries ?? 0) < 3 &&
      session.calls < config.get("agent.maxTicks") &&
      (session.tokens ?? 0) < config.get("agent.maxEpisodeTokens") &&
      Date.now() < session.started + config.get("agent.episodeTimeoutMs") &&
      unanswered.length
    ) {
      session.error = error;
      session.retries = 3;
      session.phase = "idle";
      session.next = { kind: "after", delayMs: 1 };
      session.dueAt = Date.now() + 1;
      session.retryInputs = unanswered.map((task) => task.input);
      await store.save();
      log(
        "warn",
        `tick recovery scheduled conversation=${id} tick=${session.tick} mode=answer_only reason=${error}`,
      );
      return;
    }
    session.phase = "failed";
    session.next = { kind: "wait" };
    session.error = error;
    this.jobs.cancel(id);
    for (const task of store.tasks())
      if (task.conversation === id && task.status === "running") {
        task.status = "failed";
        task.error = error;
      }
    const root = store.task(id, session.root);
    if (root) {
      root.status = "failed";
      root.error = error;
    }
    await store.save();
    log(
      "error",
      `tick failed conversation=${id} tick=${session.tick} reason=${error} unanswered=${JSON.stringify(unanswered.map((task) => task.input))}`,
    );
  }

  /** Admitted inputs this episode owns that still have no committed reply. */
  private unanswered(id: string, session: TickSession): TaskRecord[] {
    const { store } = this.host.options;
    const inputs = new Set<string>([
      session.root,
      ...session.sources.map((source) => source.input),
      ...(session.retryInputs ?? []),
    ]);
    for (const task of store.tasks())
      if (task.conversation === id && task.status === "running")
        inputs.add(task.input);
    const result: TaskRecord[] = [];
    for (const input of inputs) {
      const task = store.task(id, input);
      if (!task || task.status === "cancelled" || task.status === "answered")
        continue;
      if (task.replyHash !== undefined && task.replyHash === task.promptHash)
        continue;
      result.push(task);
    }
    return result.sort((a, b) => a.sequence - b.sequence);
  }

  private async flush(id: string): Promise<void> {
    const { store } = this.host.options;
    const conversation = this.host.snapshot?.conversations.find(
      (item) => item.id === id,
    );
    if (!conversation || this.host.stopped) return;
    for (const task of store
      .tasks()
      .filter((task) => task.conversation === id)
      .sort((a, b) => a.sequence - b.sequence)) {
      if (task.status === "cancelled") continue;
      try {
        await this.outbox.deliverParts(conversation, task, "");
      } catch (error) {
        task.status = "failed";
        task.error = error instanceof Error ? error.message : "Delivery failed";
        await store.save();
        this.host.options.log(
          "error",
          `delivery failed input=${task.input}: ${task.error}`,
        );
        return; // Preserve ordering; later messages cannot overtake this output.
      }
    }
  }
}
