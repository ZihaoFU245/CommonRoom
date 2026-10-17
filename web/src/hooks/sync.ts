// HTTP resyncs repair missed updates without letting an older response replace
// a newer WebSocket snapshot or update a session that has already signed out.
export function snapshotSync<T>(
  fetchSnapshot: () => Promise<T>,
  applySnapshot: (snapshot: T, baseline: boolean) => void,
  onError: (error: unknown) => void,
  isOlder: (incoming: T, current: T) => boolean = () => false,
) {
  let generation = 0;
  let active = true;
  let current: T | undefined;
  let pending: Promise<void> | null = null;
  return {
    receive(snapshot: T, baseline = false) {
      if (!active || (current !== undefined && isOlder(snapshot, current)))
        return;
      current = snapshot;
      generation++;
      applySnapshot(snapshot, baseline);
    },
    refresh() {
      if (!active) return Promise.resolve();
      if (pending) return pending;
      const started = generation;
      pending = Promise.resolve()
        .then(fetchSnapshot)
        .then((snapshot) => {
          if (
            !active ||
            generation !== started ||
            (current !== undefined && isOlder(snapshot, current))
          )
            return;
          current = snapshot;
          generation++;
          applySnapshot(snapshot, true);
        })
        .catch((error) => {
          if (active && generation === started) onError(error);
        })
        .finally(() => {
          pending = null;
        });
      return pending;
    },
    stop() {
      active = false;
      generation++;
    },
  };
}

export function olderSnapshot(
  incoming: { sync?: { instance: string; revision: number } },
  current: { sync?: { instance: string; revision: number } },
) {
  return (
    incoming.sync !== undefined &&
    current.sync !== undefined &&
    incoming.sync.instance === current.sync.instance &&
    incoming.sync.revision < current.sync.revision
  );
}
