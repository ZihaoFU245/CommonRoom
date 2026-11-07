/** The maximum slider position disables folding. */
export const messagePreviewMin = 256;
export const messagePreviewMax = 1024;
export const messagePreviewStep = 64;

export function normalizeMessagePreviewHeight(value: number): number {
  return Number.isFinite(value)
    ? Math.min(
        messagePreviewMax,
        Math.max(messagePreviewMin, Math.round(value)),
      )
    : messagePreviewMin;
}
