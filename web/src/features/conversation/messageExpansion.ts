/** Spend most of a long expansion revealing the part the user can actually see. */
export function messageExpansionMotion(
  from: number,
  to: number,
  visibleHeight: number,
) {
  const reveal = Math.min(to, Math.max(from + 64, visibleHeight));
  if (to <= from || reveal === to) {
    return {
      duration: 300,
      easing: "ease-out",
      frames: [
        { height: `${from}px`, offset: 0 },
        { height: `${to}px`, offset: 1 },
      ],
    };
  }
  return {
    duration: 420,
    easing: "linear",
    frames: [
      { height: `${from}px`, offset: 0, easing: "ease-in-out" },
      { height: `${reveal}px`, offset: 0.8, easing: "ease-in" },
      { height: `${to}px`, offset: 1 },
    ],
  };
}
