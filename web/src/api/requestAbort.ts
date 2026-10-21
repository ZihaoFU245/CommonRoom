/** Cancellation owned by one request, including consumption of its response body. */
export function requestAbort(timeoutMs: number, parent?: AbortSignal) {
  const timeout = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const cleanups: (() => void)[] = [];
  const dispose = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
    for (const cleanup of cleanups.splice(0)) cleanup();
  };
  let signal = timeout.signal;
  if (parent) {
    if (typeof AbortSignal.any === "function") {
      signal = AbortSignal.any([parent, timeout.signal]);
    } else {
      // Chrome 114 has AbortController and abort reasons, but no AbortSignal.any.
      const combined = new AbortController();
      const forward = (source: AbortSignal) => {
        combined.abort(source.reason);
        dispose();
      };
      for (const source of [parent, timeout.signal]) {
        if (source.aborted) {
          forward(source);
          break;
        }
        const abort = () => forward(source);
        source.addEventListener("abort", abort, { once: true });
        cleanups.push(() => source.removeEventListener("abort", abort));
      }
      signal = combined.signal;
    }
  }
  if (!signal.aborted) {
    // Native timeout() reports AbortError on Chrome 114. Own the timer so all
    // browsers use TimeoutError and completion releases it immediately.
    timer = setTimeout(() => {
      timeout.abort(new DOMException("The request timed out.", "TimeoutError"));
      dispose();
    }, timeoutMs);
    const abort = () => dispose();
    signal.addEventListener("abort", abort, { once: true });
    cleanups.push(() => signal.removeEventListener("abort", abort));
  }
  return { signal, dispose };
}
