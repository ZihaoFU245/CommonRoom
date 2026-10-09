import { useLayoutEffect, useRef, useState } from "preact/hooks";

/** Keep the picker inside the visible transcript, including while scrolling. */
export function useReactionPlacement(open: boolean) {
  const anchor = useRef<HTMLDetailsElement>(null);
  const picker = useRef<HTMLDivElement>(null);
  const [above, setAbove] = useState(false);

  useLayoutEffect(() => {
    if (!open || !anchor.current || !picker.current) return;
    const trigger = anchor.current;
    const panel = picker.current;
    const transcript = trigger.closest<HTMLElement>(".message-list");
    const place = () => {
      const bounds = trigger.getBoundingClientRect();
      const clip = transcript?.getBoundingClientRect();
      const top = Math.max(0, clip ? clip.top + transcript!.clientTop : 0);
      const bottom = Math.min(
        window.innerHeight,
        clip
          ? clip.top + transcript!.clientTop + transcript!.clientHeight
          : window.innerHeight,
      );
      const below = bottom - bounds.bottom - 4;
      const above = bounds.top - top - 4;
      setAbove(below < panel.offsetHeight && above > below);
    };
    place();
    const observer = new ResizeObserver(place);
    observer.observe(panel);
    if (transcript) observer.observe(transcript);
    transcript?.addEventListener("scroll", place, { passive: true });
    window.addEventListener("resize", place);
    return () => {
      observer.disconnect();
      transcript?.removeEventListener("scroll", place);
      window.removeEventListener("resize", place);
    };
  }, [open]);

  return { anchor, picker, above };
}
