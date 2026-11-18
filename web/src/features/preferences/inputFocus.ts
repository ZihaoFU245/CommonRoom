export const touchControls = "(hover: none) and (pointer: coarse)";
// Short touch viewports keep the phone layout in landscape.
export const mobileLayout = `(max-width: 700px), ${touchControls} and (max-height: 500px)`;

const textInputTypes = new Set([
  "email",
  "number",
  "password",
  "search",
  "tel",
  "text",
  "url",
]);
let lastPointerWasTouch = false;

export function prefersManualTextInput() {
  return (
    lastPointerWasTouch ||
    matchMedia(`(max-width: 700px), ${touchControls}`).matches
  );
}

/** Remember the input used on hybrids where the primary pointer may be a mouse. */
export function trackFocusInput() {
  const update = (event: PointerEvent) => {
    if (event.isPrimary) lastPointerWasTouch = event.pointerType === "touch";
  };
  document.addEventListener("pointerdown", update, {
    capture: true,
    passive: true,
  });
  return () => {
    document.removeEventListener("pointerdown", update, true);
    lastPointerWasTouch = false;
  };
}

/** Programmatic focus keeps touch and compact text entry opt-in. */
export function focusControl(
  element: HTMLElement | null | undefined,
  options?: FocusOptions,
) {
  if (!element) return false;
  const editable =
    element.isContentEditable ||
    (element instanceof HTMLTextAreaElement && !element.readOnly) ||
    (element instanceof HTMLInputElement &&
      !element.readOnly &&
      textInputTypes.has(element.type));
  if (editable && prefersManualTextInput()) return false;
  element.focus(options);
  return true;
}
