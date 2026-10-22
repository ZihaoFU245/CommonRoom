/** Transformed rows can temporarily extend scrollHeight during bottom-following
 * motion. Only manual input should release that intent before motion settles.
 */
export function timelineAtBottom(
  viewport: HTMLElement,
  following: boolean,
  threshold = 32,
) {
  return (
    (following &&
      viewport.querySelector(
        ".timeline-rows.is-animating, [data-content-resizing]",
      ) !== null) ||
    viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight <
      threshold
  );
}
