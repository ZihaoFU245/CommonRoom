import { flushSync } from "preact/compat";

let navigation = 0;
let active: ViewTransition | undefined;

/** Animate app navigation and commit system navigation before the next paint. */
export function animateNavigation(
  direction: "forward" | "back" | undefined,
  update: () => void,
  immediate = false,
) {
  const current = ++navigation;
  if (immediate || active) {
    // Do not start another snapshot while the previous one is still settling.
    active?.skipTransition();
    flushSync(update);
    return;
  }
  if (
    !direction ||
    !document.startViewTransition ||
    !CSS.supports("selector(:active-view-transition-type(mobile-forward))") ||
    matchMedia("(prefers-reduced-motion: reduce)").matches
  ) {
    update();
    return;
  }
  let committed = false;
  const commit = () => {
    if (committed || current !== navigation) return;
    committed = true;
    flushSync(update);
  };
  const transition = (active = document.startViewTransition({
    types: [`mobile-${direction}`],
    update: () => {
      window.clearTimeout(captureTimeout);
      commit();
    },
  }));
  const captureTimeout = window.setTimeout(() => {
    if (current !== navigation || committed) return;
    // A slow snapshot should not hold the route change behind a frozen frame.
    transition.skipTransition();
    commit();
  }, 100);
  void transition.ready.catch(() => {
    // A newer navigation or a hidden document can skip capture.
  });
  const clear = () => {
    if (active === transition) active = undefined;
  };
  void transition.finished.then(clear, (error: unknown) => {
    clear();
    console.error("Navigation transition failed", error);
  });
}
