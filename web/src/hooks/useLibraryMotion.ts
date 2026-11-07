import { useLayoutEffect, useRef } from "preact/hooks";
import { controlMotion } from "../features/preferences/motion.ts";
import { messageMotion } from "../features/conversation/timelineMotion.ts";

interface Bounds {
  left: number;
  top: number;
  width: number;
  height: number;
}
interface Tile {
  bounds: Bounds;
  from: Bounds;
  opacity: number;
  motion?: Animation;
}

function sameBounds(before: Bounds, after: Bounds) {
  return (
    Math.abs(before.left - after.left) < 0.5 &&
    Math.abs(before.top - after.top) < 0.5 &&
    Math.abs(before.width - after.width) < 0.5 &&
    Math.abs(before.height - after.height) < 0.5
  );
}

/** One layout pass owns row wrapping, image sizing, additions and removals.
 * Animate painted boxes, never flex dimensions: exiting previews cannot become
 * flex items again or displace Add. Retarget interrupted motion from its paint.
 */
export function useLibraryMotion(mode: string) {
  const content = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const root = content.current;
    if (!root) return;
    const reduced = matchMedia("(prefers-reduced-motion: reduce)");
    let previous = new Map<HTMLElement, Tile>();
    let scrollTop = root.scrollTop;
    const scrolled = () => {
      scrollTop = root.scrollTop;
    };
    root.addEventListener("scroll", scrolled);
    const exits = new Map<HTMLElement, Animation>();
    const selector =
      ".library-grid > li, .library-section-heading, .library-section-status, .library-pagination";
    const painted = (tile: Tile) => {
      const amount = tile.motion?.effect?.getComputedTiming().progress ?? 1;
      const mix = (start: number, end: number) =>
        start + (end - start) * amount;
      return {
        bounds: {
          left: mix(tile.from.left, tile.bounds.left),
          top: mix(tile.from.top, tile.bounds.top),
          width: mix(tile.from.width, tile.bounds.width),
          height: mix(tile.from.height, tile.bounds.height),
        },
        opacity: mix(tile.opacity, 1),
      };
    };
    const sync = () => {
      if (reduced.matches) {
        for (const [ghost, motion] of exits) {
          motion.cancel();
          ghost.remove();
        }
        exits.clear();
      }
      const elements = Array.from(
        root.querySelectorAll<HTMLElement>(selector),
      ).filter((element) => !element.closest(".library-motion-exit"));
      const scrollDelta = root.scrollTop - scrollTop;
      scrollTop = root.scrollTop;
      const before = new Map(
        Array.from(previous, ([element, tile]) => [element, painted(tile)]),
      );
      for (const paint of before.values()) paint.bounds.top += scrollDelta;
      const origin = root.getBoundingClientRect();
      // Normalize the popover's opening scale independently of tile motion.
      const scale = origin.width / root.offsetWidth;
      // Read all target boxes before starting effects. Subtract our current
      // transform so measurement never needs to cancel a running animation.
      const measured = elements.map((element) => {
        const rect = element.getBoundingClientRect();
        const prior = previous.get(element);
        const paint = before.get(element);
        const shiftX =
          prior && paint ? paint.bounds.left - prior.bounds.left : 0;
        const shiftY =
          prior && paint
            ? paint.bounds.top - prior.bounds.top - scrollDelta
            : 0;
        const scaleX =
          prior?.motion && prior.bounds.width && paint
            ? paint.bounds.width / prior.bounds.width
            : 1;
        const scaleY =
          prior?.motion && prior.bounds.height && paint
            ? paint.bounds.height / prior.bounds.height
            : 1;
        return {
          element,
          bounds: {
            left: (rect.left - origin.left) / scale + root.scrollLeft - shiftX,
            top: (rect.top - origin.top) / scale + root.scrollTop - shiftY,
            width: rect.width / scale / scaleX,
            height: rect.height / scale / scaleY,
          },
        };
      });
      const live = new Set(elements);
      const removing = Array.from(previous.keys()).some(
        (element) => !live.has(element),
      );
      const next = new Map<HTMLElement, Tile>();
      for (const { element, bounds } of measured) {
        const prior = previous.get(element);
        if (
          !reduced.matches &&
          prior &&
          !scrollDelta &&
          sameBounds(prior.bounds, bounds)
        ) {
          // A load callback or unrelated DOM update must not extend the motion.
          next.set(element, prior);
          continue;
        }
        prior?.motion?.cancel();
        const oldPaint = before.get(element);
        const old = oldPaint?.bounds;
        const opacity = oldPaint?.opacity ?? (previous.size ? 0 : 1);
        const tile: Tile = { bounds, from: old ?? bounds, opacity };
        next.set(element, tile);
        if (reduced.matches || !bounds.width || !bounds.height) continue;
        if (old) {
          if (sameBounds(old, bounds) && opacity === 1) continue;
          tile.motion = element.animate(
            [
              {
                opacity,
                transformOrigin: "top left",
                transform: `translate(${old.left - bounds.left}px, ${old.top - bounds.top}px) scale(${old.width / bounds.width}, ${old.height / bounds.height})`,
              },
              { opacity: 1, transformOrigin: "top left", transform: "none" },
            ],
            removing ? messageMotion : controlMotion,
          );
        } else if (previous.size) {
          tile.motion = element.animate(
            [{ opacity: 0 }, { opacity: 1 }],
            controlMotion,
          );
        }
      }
      for (const [element, tile] of previous) {
        if (next.has(element)) continue;
        resize.unobserve(element);
        tile.motion?.cancel();
        if (reduced.matches) continue;
        const paint = before.get(element);
        if (!paint) continue;
        const { bounds, opacity } = paint;
        const ghost = element.cloneNode(true);
        if (!(ghost instanceof HTMLElement)) continue;
        ghost.classList.add("library-motion-exit");
        ghost.setAttribute("aria-hidden", "true");
        ghost.inert = true;
        Object.assign(ghost.style, {
          left: `${bounds.left}px`,
          top: `${bounds.top}px`,
          width: `${bounds.width}px`,
          height: `${bounds.height}px`,
          margin: "0",
          minWidth: "0",
          maxWidth: "none",
        });
        root.append(ghost);
        const motion = ghost.animate(
          [{ opacity }, { opacity: 0 }],
          messageMotion,
        );
        exits.set(ghost, motion);
        motion.addEventListener(
          "finish",
          () => {
            ghost.remove();
            exits.delete(ghost);
          },
          { once: true },
        );
      }
      for (const element of elements) {
        if (!previous.has(element)) resize.observe(element);
      }
      previous = next;
    };
    const resize = new ResizeObserver(sync);
    const mutations = new MutationObserver((records) => {
      // Ignore our own exit layer. It is outside the live layout and selectors.
      if (
        records.some(
          (record) =>
            record.target !== root ||
            [...record.addedNodes, ...record.removedNodes].some(
              (node) =>
                !(
                  node instanceof HTMLElement &&
                  node.classList.contains("library-motion-exit")
                ),
            ),
        )
      )
        sync();
    });
    sync();
    resize.observe(root);
    mutations.observe(root, { childList: true, subtree: true });
    reduced.addEventListener("change", sync);
    return () => {
      root.removeEventListener("scroll", scrolled);
      resize.disconnect();
      mutations.disconnect();
      reduced.removeEventListener("change", sync);
      for (const tile of previous.values()) tile.motion?.cancel();
      for (const [ghost, motion] of exits) {
        motion.cancel();
        ghost.remove();
      }
    };
  }, [mode]);
  return content;
}
