import { useRef, useCallback } from "preact/hooks";
/** Stable callbacks see current state without re-rendering the transcript on typing. */
export function useEvent<Args extends unknown[], Result>(
  handler: (...args: Args) => Result,
) {
  const current = useRef(handler);
  current.current = handler;
  return useCallback((...args: Args) => current.current(...args), []);
}
