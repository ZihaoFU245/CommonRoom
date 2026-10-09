// HTTP resyncs repair missed updates without letting an older response replace
// a newer WebSocket snapshot or update a session that has already signed out.
export function snapshotSync<T>(
  fetchSnapshot: () => Promise<T>,
  applySnapshot: (snapshot: T, baseline: boolean) => void,
  onError: (error: unknown) => void,
) {
  let generation = 0;
  let active = true;
  let pending: Promise<void> | null = null;
  return {
    receive(snapshot: T, baseline = false) {
      if (!active) return;
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
          if (!active || generation !== started) return;
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
