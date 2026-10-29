/** Read a cached range with bounded buffering instead of materializing a file. */
export function rangeStream(
  body: ReadableStream<Uint8Array>,
  start: number,
  end: number,
  signal: AbortSignal,
): ReadableStream<Uint8Array> {
  const reader = body.getReader();
  let position = 0;
  let finished = false;
  const abort = () => {
    if (!finished) reader.cancel(signal.reason).catch(() => {});
  };
  signal.addEventListener("abort", abort, { once: true });
  function cleanup() {
    finished = true;
    signal.removeEventListener("abort", abort);
  }
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        signal.throwIfAborted();
        while (position <= end) {
          const { done, value } = await reader.read();
          signal.throwIfAborted();
          if (done) {
            cleanup();
            controller.error(
              new Error("Cached file ended before the requested range."),
            );
            return;
          }
          const from = Math.max(0, start - position);
          const to = Math.min(value.byteLength, end + 1 - position);
          position += value.byteLength;
          if (to > from) controller.enqueue(value.subarray(from, to));
          if (position > end) {
            cleanup();
            controller.close();
            await reader.cancel();
          }
          if (to > from) return;
        }
      } catch (error) {
        cleanup();
        controller.error(error);
        await reader.cancel().catch(() => {});
      }
    },
    async cancel(reason: unknown) {
      cleanup();
      await reader.cancel(reason);
    },
  });
}
