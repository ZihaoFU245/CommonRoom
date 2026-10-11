export function indexAt(offsets: number[], position: number) {
  let low = 0;
  let high = Math.max(0, offsets.length - 1);
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if ((offsets[mid] ?? 0) <= position) low = mid;
    else high = mid - 1;
  }
  return low;
}
export function timelineOffsets(keys: string[], sizes: Map<string, number>) {
  const offsets = [0];
  for (const key of keys)
    offsets.push((offsets.at(-1) ?? 0) + (sizes.get(key) ?? 100));
  return offsets;
}
export function virtualWindow(offsets: number[], top: number, height: number) {
  const count = Math.max(0, offsets.length - 1);
  const start = Math.max(0, indexAt(offsets, top) - 8);
  const end = Math.min(count, indexAt(offsets, top + height) + 9);
  return {
    start,
    end,
    top: offsets[start] ?? 0,
    bottom: Math.max(0, (offsets.at(-1) ?? 0) - (offsets[end] ?? 0)),
  };
}
export function anchoredScrollTop(
  oldKeys: string[],
  oldOffsets: number[],
  keys: string[],
  offsets: number[],
  top: number,
) {
  const index = indexAt(oldOffsets, top);
  const key = oldKeys[index];
  const next = keys.findIndex((entry) => entry === key);
  return next < 0 ? top : (offsets[next] ?? 0) + top - (oldOffsets[index] ?? 0);
}
