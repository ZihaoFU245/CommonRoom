/** Transformed rows can temporarily extend scrollHeight during bottom-following
 * motion. Manual input releases that ownership before its scroll event.
 */
export function timelineAtBottom(
  viewport: HTMLElement,
  following: boolean,
  previousTop?: number,
  threshold = 32,
) {
  if (
    following &&
    viewport.querySelector(
      ".timeline-rows.is-animating, [data-content-resizing]",
    ) !== null
  )
    return true;
  const gap =
    viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight;
  if (gap <= 0) return true;
  // Even a subpixel upward movement must escape bottom following. Otherwise
  // media measurements can snap each small delta back inside the threshold.
  if (previousTop !== undefined && viewport.scrollTop < previousTop)
    return false;
  // Browsers may deliver another scroll event for the same position.
  if (!following && viewport.scrollTop === previousTop) return false;
  return gap < threshold;
}
