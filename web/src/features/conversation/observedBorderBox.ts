/** Message wrappers use horizontal writing and need border-box layout sizes.
 * ContentRect omits padding/borders, so older implementations use a rect fallback.
 */
export function observedBorderBox(entry: ResizeObserverEntry) {
  const box = entry.borderBoxSize?.[0];
  return box &&
    Number.isFinite(box.inlineSize) &&
    Number.isFinite(box.blockSize)
    ? { width: box.inlineSize, height: box.blockSize }
    : entry.target.getBoundingClientRect();
}
