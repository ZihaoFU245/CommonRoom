import { useEvent } from "./useEvent.ts";
import type { JSX } from "preact";
import { useEffect, useRef } from "preact/hooks";

/** Keep IME candidate keys out of composer shortcuts, including Safari's late Enter. */
export function useCompositionGuard(
  keydown: JSX.KeyboardEventHandler<HTMLTextAreaElement>,
) {
  const composing = useRef(false);
  const resetTimer = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  function cancelReset() {
    clearTimeout(resetTimer.current);
    resetTimer.current = undefined;
  }
  const reset = useEvent(() => {
    cancelReset();
    composing.current = false;
  });
  useEffect(() => reset, [reset]);
  return {
    onCompositionStart: () => {
      cancelReset();
      composing.current = true;
    },
    onCompositionEnd: () => {
      cancelReset();
      // Safari can dispatch the confirming Enter after compositionend, with
      // isComposing already false. Keep it native until the next event-loop turn.
      composing.current = true;
      resetTimer.current = setTimeout(reset, 0);
    },
    onBlur: reset,
    onKeyDown: (event: JSX.TargetedKeyboardEvent<HTMLTextAreaElement>) => {
      if (event.isComposing || event.keyCode === 229 || composing.current)
        return;
      keydown(event);
    },
  };
}
