export interface ToolAccess {
  resource: string;
  mode: "read" | "write";
}

interface Waiter {
  mode: ToolAccess["mode"];
  grant(release: () => void): void;
}

interface Lock {
  readers: number;
  writer: boolean;
  queue: Waiter[];
}

/** FIFO admission: readers may overlap, but never jump a queued writer. */
export class ToolLocks {
  private readonly locks = new Map<string, Lock>();

  async run<T>(
    access: ToolAccess,
    signal: AbortSignal,
    work: () => Promise<T>,
  ): Promise<T> {
    const release = await this.acquire(access, signal);
    try {
      signal.throwIfAborted();
      return await work();
    } finally {
      release();
    }
  }

  private acquire(
    access: ToolAccess,
    signal: AbortSignal,
  ): Promise<() => void> {
    signal.throwIfAborted();
    let lock = this.locks.get(access.resource);
    if (!lock) {
      lock = { readers: 0, writer: false, queue: [] };
      this.locks.set(access.resource, lock);
    }
    const state = lock;
    return new Promise((resolve, reject) => {
      const cancel = () => {
        const index = state.queue.indexOf(waiter);
        if (index < 0) return;
        state.queue.splice(index, 1);
        signal.removeEventListener("abort", cancel);
        reject(signal.reason);
        this.drain(access.resource, state);
      };
      const waiter: Waiter = {
        mode: access.mode,
        grant: (release) => {
          signal.removeEventListener("abort", cancel);
          resolve(release);
        },
      };
      state.queue.push(waiter);
      signal.addEventListener("abort", cancel, { once: true });
      if (signal.aborted) cancel();
      else this.drain(access.resource, state);
    });
  }

  private drain(resource: string, lock: Lock): void {
    while (!lock.writer && lock.queue.length) {
      const next = lock.queue[0];
      if (!next || (next.mode === "write" && lock.readers)) break;
      lock.queue.shift();
      if (next.mode === "write") lock.writer = true;
      else lock.readers++;
      let released = false;
      next.grant(() => {
        if (released) return;
        released = true;
        if (next.mode === "write") lock.writer = false;
        else lock.readers--;
        this.drain(resource, lock);
      });
    }
    if (!lock.writer && !lock.readers && !lock.queue.length)
      this.locks.delete(resource);
  }
}
