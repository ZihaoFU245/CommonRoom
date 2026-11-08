/** Shared timing for small controls, menus and settings reveals. */
export const controlMotion = { duration: 120, easing: "ease-out" } as const;

/** Give lifted objects and the slots around them time to settle. */
export const reorderMotion = {
  duration: 180,
  easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
} as const;
