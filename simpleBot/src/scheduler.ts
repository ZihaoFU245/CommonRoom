/** Deduplicated FIFO scheduling. Waiting conversations occupy no request slot. */
export class Scheduler {
  private readonly queue = new Set<string>();
  private readonly running = new Map<string, Promise<void>>();
  private readonly timers = new Map<string, ReturnType<typeof setTimeout>>();
  private readonly run: (conversation: string) => Promise<void>;
  private readonly error: (error: unknown) => void;
  private readonly capacity: () => number;
  private closed = false;
  private scheduled = false;

  constructor(
    run: (conversation: string) => Promise<void>,
    capacity: () => number,
    error: (error: unknown) => void,
  ) {
    this.run = run;
    this.capacity = capacity;
    this.error = error;
  }

  get busy(): boolean {
    return this.queue.size > 0 || this.running.size > 0;
  }

  wake(id: string): void {
    if (this.closed) return;
    this.queue.add(id);
    this.schedule();
  }

  at(id: string, deadline: number): void {
    this.cancelTimer(id);
    if (this.closed) return;
    this.timers.set(
      id,
      setTimeout(
        () => {
          this.timers.delete(id);
          this.wake(id);
        },
        Math.max(1, deadline - Date.now()),
      ),
    );
  }

  cancelTimer(id: string): void {
    const timer = this.timers.get(id);
    if (timer) clearTimeout(timer);
    this.timers.delete(id);
  }

  private schedule(): void {
    if (this.scheduled || this.closed) return;
    this.scheduled = true;
    queueMicrotask(() => {
      this.scheduled = false;
      this.pump();
    });
  }

  private pump(): void {
    if (this.closed) return;
    for (const id of this.queue) {
      if (this.running.size >= this.capacity()) break;
      if (this.running.has(id)) continue;
      this.queue.delete(id);
      const work = this.run(id)
        .catch(this.error)
        .finally(() => {
          this.running.delete(id);
          this.schedule();
        });
      this.running.set(id, work);
    }
  }

  /** Await runnable work only; future deadlines and background jobs stay asleep. */
  async settle(): Promise<void> {
    while (this.busy) {
      this.pump();
      const running = [...this.running.values()];
      if (running.length) await Promise.all(running);
      else await Promise.resolve();
    }
  }

  async close(): Promise<void> {
    this.closed = true;
    this.queue.clear();
    for (const id of this.timers.keys()) this.cancelTimer(id);
    await Promise.all(this.running.values());
  }
}
