import { flushSync } from "preact/compat";

let navigation = 0;
let active: ViewTransition | undefined;

/** Commit mobile navigation inside the browser's snapshot update. */
export function animateNavigation(
  direction: "forward" | "back" | undefined,
  update: () => void,
) {
  const current = ++navigation;
  active?.skipTransition();
  if (
    !direction ||
    !document.startViewTransition ||
    !CSS.supports("selector(:active-view-transition-type(mobile-forward))") ||
    matchMedia("(prefers-reduced-motion: reduce)").matches
  ) {
    update();
    return;
  }
  const transition = (active = document.startViewTransition({
    types: [`mobile-${direction}`],
    update: () => {
      // Skipped transitions still run their update callbacks.
      if (current === navigation) flushSync(update);
    },
  }));
  void transition.ready.catch(() => {
    // A newer navigation or a hidden document can skip capture.
  });
  void transition.finished
    .catch((error: unknown) => {
      console.error("Navigation transition failed", error);
    })
    .finally(() => {
      if (active === transition) active = undefined;
    });
}
