/** Presentation config only. Uploads always authorize on the server. */
interface Configuration {
  enabled: boolean;
  max_size: number;
}
interface Pending {
  controller: AbortController;
  readers: number;
  promise: Promise<Configuration>;
}
interface Entry {
  base: string;
  value?: Configuration;
  pending?: Pending;
}
class MediaConfigCache {
  private entry: Entry | undefined;
  private instance: { base: string; value: string } | undefined;

  clear(): void {
    const previous = this.entry;
    this.entry = undefined;
    this.instance = undefined;
    previous?.pending?.controller.abort();
  }
  observeInstance(base: string, instance: string | undefined): void {
    if (instance === undefined) return;
    if (
      this.instance &&
      (this.instance.base !== base || this.instance.value !== instance)
    )
      this.clear();
    this.instance = { base, value: instance };
  }
  async load(
    base: string,
    signal: AbortSignal,
    fetch: (signal: AbortSignal) => Promise<Configuration>,
  ): Promise<Configuration> {
    signal.throwIfAborted();
    if (this.entry && this.entry.base !== base) this.clear();
    const entry = this.entry ?? { base };
    this.entry = entry;
    if (entry.value) return { ...entry.value };
    if (!entry.pending) {
      const controller = new AbortController();
      const pending: Pending = {
        controller,
        readers: 0,
        promise: fetch(controller.signal).then(
          (value) => {
            if (this.entry !== entry)
              throw new DOMException(
                "Configuration context changed",
                "AbortError",
              );
            entry.value = value;
            delete entry.pending;
            return value;
          },
          (error: unknown) => {
            if (this.entry === entry) this.entry = undefined;
            throw error;
          },
        ),
      };
      entry.pending = pending;
    }
    const pending = entry.pending;
    pending.readers++;
    let abort = () => {};
    try {
      signal.throwIfAborted();
      const value = await Promise.race([
        pending.promise,
        new Promise<undefined>((resolve) => {
          abort = () => resolve(undefined);
          signal.addEventListener("abort", abort, { once: true });
        }),
      ]);
      signal.throwIfAborted();
      if (value === undefined)
        throw new DOMException("Configuration read aborted", "AbortError");
      return { ...value };
    } finally {
      signal.removeEventListener("abort", abort);
      if (--pending.readers === 0 && entry.pending === pending) {
        if (this.entry === entry) this.entry = undefined;
        pending.controller.abort();
      }
    }
  }
}
export const mediaConfigCache = new MediaConfigCache();
