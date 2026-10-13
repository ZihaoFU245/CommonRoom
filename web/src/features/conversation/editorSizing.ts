/** Keep the editor footer at the same viewport position after a height change. */
export function editorScrollTop(
  previousTop: number,
  previousBottom: number,
  bottom: number,
  maximumTop: number,
) {
  return Math.max(
    0,
    Math.min(maximumTop, previousTop + bottom - previousBottom),
  );
}
