import { useEffect, useRef, useState } from "preact/hooks";
import type { JSX, RefObject } from "preact";
import type { LibraryItem } from "../api/library.ts";
import { api } from "../api/client.ts";
import { errorMessage } from "../api/protocol.ts";
import {
  controlMotion,
  reorderMotion,
} from "../features/preferences/motion.ts";
import { useEvent } from "./useEvent.ts";

interface Drag {
  id: string;
  pointer: number;
  x: number;
  y: number;
  startX: number;
  startY: number;
  offsetX: number;
  offsetY: number;
  source: HTMLElement;
  preview: HTMLElement | null;
  frame: number;
  changed: boolean;
  moved: boolean;
  order: string[];
  original: string[];
  hover: string | null;
  hoverSince: number;
  edgeDirection: number;
  edgeSince: number;
  lastFrame: number;
  dispose: () => void;
}

function tileBounds(tile: HTMLElement, root: HTMLElement) {
  let left = 0;
  let top = 0;
  let node: HTMLElement | null = tile;
  while (node && node !== root) {
    left += node.offsetLeft;
    top += node.offsetTop;
    node = node.offsetParent instanceof HTMLElement ? node.offsetParent : null;
  }
  const rect = root.getBoundingClientRect();
  const scale = rect.width / root.offsetWidth;
  return {
    left: rect.left + (left - root.scrollLeft) * scale,
    top: rect.top + (top - root.scrollTop) * scale,
    width: tile.offsetWidth * scale,
    height: tile.offsetHeight * scale,
  };
}

/** Pointer motion owns only the floating preview. Keyed layout changes stay
 * with useLibraryMotion, including interrupted wrapping and resize effects.
 */
export function useLibraryReorder(
  root: RefObject<HTMLDivElement>,
  items: LibraryItem[],
  enabled: boolean,
) {
  const [order, setOrder] = useState<string[] | null>(null);
  const [saving, setSaving] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState("");
  const active = useRef<Drag | null>(null);
  const settling = useRef<(() => void) | null>(null);
  const mounted = useRef(true);
  const pending = useRef(false);
  const source = useRef(items);
  const ordered = order
    ? order.flatMap((id) => items.filter((item) => item.media_id === id))
    : items;

  const cancel = useEvent(() => {
    const drag = active.current;
    if (!drag) return false;
    active.current = null;
    drag.dispose();
    if (drag.preview) settle(drag);
    else {
      drag.source.removeAttribute("data-library-drag-source");
      setDragging(false);
    }
    setOrder(null);
    return true;
  });

  useEffect(() => {
    if (source.current !== items) {
      source.current = items;
      if (!pending.current) {
        cancel();
        setOrder(null);
      }
    }
    if (!enabled) cancel();
  }, [items, enabled, cancel]);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      const drag = active.current;
      drag?.dispose();
      drag?.preview?.remove();
      drag?.source.removeAttribute("data-library-drag-source");
      active.current = null;
      settling.current?.();
    };
  }, []);

  const save = useEvent(
    async (id: string, ids: string[], keyboardFocus?: HTMLButtonElement) => {
      const index = ids.indexOf(id);
      const before = ids[index + 1];
      const after = ids[index - 1];
      if (!before && !after) return;
      pending.current = true;
      setSaving(true);
      setError("");
      try {
        await api.libraryMove(
          id,
          before ? { before_id: before } : { after_id: after ?? "" },
        );
      } catch (failure) {
        if (mounted.current) {
          setError(errorMessage(failure));
          setOrder(null);
        }
      } finally {
        pending.current = false;
        if (mounted.current) setSaving(false);
        // Refresh all pages and the account's other mounted pickers. Server
        // notifications provide the same invalidation to other tabs/devices.
        window.dispatchEvent(new Event("commonroom-library-change"));
        if (keyboardFocus && mounted.current)
          requestAnimationFrame(() => {
            // Disabling a button during persistence drops browser focus. Restore
            // it after success/rollback, unless the user has focused another control.
            if (
              mounted.current &&
              keyboardFocus.isConnected &&
              !keyboardFocus.disabled &&
              !keyboardFocus.closest("[inert]") &&
              document.activeElement === document.body
            )
              keyboardFocus.focus({ preventScroll: true });
          });
      }
    },
  );

  const settle = useEvent((drag: Drag) => {
    const preview = drag.preview;
    if (!preview) return;
    settling.current?.();
    // The lift scales around the grab point. Convert its painted box back to
    // viewport coordinates before settling, so removing that scale never jumps.
    const lifted = preview.getBoundingClientRect();
    for (const animation of preview.getAnimations()) animation.cancel();
    Object.assign(preview.style, {
      transform: "none",
      left: `${lifted.left}px`,
      top: `${lifted.top}px`,
      width: `${lifted.width}px`,
      height: `${lifted.height}px`,
    });
    let frame = 0;
    let motion: Animation | null = null;
    let destination = "";
    const reduced = matchMedia("(prefers-reduced-motion: reduce)");
    const finish = () => {
      cancelAnimationFrame(frame);
      motion?.cancel();
      preview.remove();
      drag.source.removeAttribute("data-library-drag-source");
      reduced.removeEventListener("change", finish);
      if (settling.current === finish) settling.current = null;
      if (mounted.current) setDragging(false);
    };
    settling.current = finish;
    reduced.addEventListener("change", finish);
    const follow = () => {
      if (!drag.source.isConnected || reduced.matches) {
        finish();
        return;
      }
      const target = drag.source.getBoundingClientRect();
      const next = [target.left, target.top, target.width, target.height]
        .map(Math.round)
        .join(":");
      if (next !== destination) {
        const painted = preview.getBoundingClientRect();
        motion?.cancel();
        destination = next;
        motion = preview.animate(
          [
            {
              left: `${painted.left}px`,
              top: `${painted.top}px`,
              width: `${painted.width}px`,
              height: `${painted.height}px`,
            },
            {
              left: `${target.left}px`,
              top: `${target.top}px`,
              width: `${target.width}px`,
              height: `${target.height}px`,
            },
          ],
          { ...reorderMotion, fill: "forwards" },
        );
      } else if (motion?.playState === "finished") {
        finish();
        return;
      }
      frame = requestAnimationFrame(follow);
    };
    frame = requestAnimationFrame(follow);
  });

  const pointerDown = useEvent(
    (event: JSX.TargetedPointerEvent<HTMLButtonElement>, id: string) => {
      const content = root.current;
      const tile = event.currentTarget.closest("li");
      if (
        !enabled ||
        pending.current ||
        active.current ||
        settling.current ||
        !content ||
        !(tile instanceof HTMLElement) ||
        event.button !== 0 ||
        !event.isPrimary
      )
        return;
      event.preventDefault();
      event.currentTarget.focus({ preventScroll: true });
      // Native capture keeps a release outside the tile/window from stranding
      // the drag. Scripted PointerEvents have no active browser pointer ID.
      if (event.isTrusted) content.setPointerCapture(event.pointerId);
      const bounds = tile.getBoundingClientRect();
      const ids = ordered.map((item) => item.media_id);
      const drag: Drag = {
        id,
        pointer: event.pointerId,
        x: event.clientX,
        y: event.clientY,
        startX: event.clientX,
        startY: event.clientY,
        offsetX: event.clientX - bounds.left,
        offsetY: event.clientY - bounds.top,
        source: tile,
        preview: null,
        frame: 0,
        changed: false,
        moved: false,
        order: ids,
        original: ids.slice(),
        hover: null,
        hoverSince: 0,
        edgeDirection: 0,
        edgeSince: 0,
        lastFrame: performance.now(),
        dispose: () => {},
      };
      active.current = drag;
      let dropping = false;
      const update = () => {
        if (active.current !== drag) return;
        if (!drag.preview) {
          if (Math.hypot(drag.x - drag.startX, drag.y - drag.startY) < 5)
            return;
          const clone = tile.cloneNode(true);
          if (!(clone instanceof HTMLElement)) return;
          clone.classList.add("library-drag-preview");
          clone.setAttribute("aria-hidden", "true");
          clone.inert = true;
          Object.assign(clone.style, {
            left: `${bounds.left}px`,
            top: `${bounds.top}px`,
            width: `${bounds.width}px`,
            height: `${bounds.height}px`,
            transformOrigin: `${drag.offsetX}px ${drag.offsetY}px`,
          });
          // A transformed picker establishes a containing block even for fixed
          // descendants. Give the lifted object its own top-layer surface so
          // its coordinates really are viewport coordinates, with no clipping.
          clone.setAttribute("popover", "manual");
          document.body.append(clone);
          if (typeof clone.showPopover === "function") clone.showPopover();
          if (!matchMedia("(prefers-reduced-motion: reduce)").matches)
            clone.animate(
              [{ transform: "scale(1)" }, { transform: "scale(1.045)" }],
              { ...controlMotion, fill: "forwards" },
            );
          drag.preview = clone;
          tile.setAttribute("data-library-drag-source", "");
          setDragging(true);
          setError("");
        }
        drag.preview.style.left = `${drag.x - drag.offsetX}px`;
        drag.preview.style.top = `${drag.y - drag.offsetY}px`;
        const viewport = content.getBoundingClientRect();
        const oldScroll = content.scrollTop;
        const now = performance.now();
        const elapsed = Math.min(32, now - drag.lastFrame);
        drag.lastFrame = now;
        // Passing through a bottom-row sticker must not scroll the panel.
        // Require a deliberate pull outside its edge, held for 350ms.
        const edge =
          drag.x >= viewport.left && drag.x <= viewport.right
            ? drag.y < viewport.top - 12
              ? -1
              : drag.y > viewport.bottom + 12
                ? 1
                : 0
            : 0;
        if (edge !== drag.edgeDirection) {
          drag.edgeDirection = edge;
          drag.edgeSince = now;
        }
        if (!dropping && edge && now - drag.edgeSince >= 350) {
          const distance =
            edge < 0 ? viewport.top - drag.y : drag.y - viewport.bottom;
          content.scrollTop +=
            (edge * Math.min(240, 90 + distance * 3) * elapsed) / 1000;
        }
        if (
          drag.moved ||
          drag.hover !== null ||
          oldScroll !== content.scrollTop
        ) {
          drag.moved = false;
          const centerX = drag.x - drag.offsetX + bounds.width / 2;
          const centerY = drag.y - drag.offsetY + bounds.height / 2;
          const candidates = Array.from(
            content.querySelectorAll<HTMLElement>("[data-library-reorder-id]"),
          );
          const target = candidates.find((candidate) => {
            if (candidate === tile) return false;
            const box = tileBounds(candidate, content);
            return (
              centerX >= box.left + box.width * 0.2 &&
              centerX <= box.left + box.width * 0.8 &&
              centerY >= box.top + box.height * 0.2 &&
              centerY <= box.top + box.height * 0.8
            );
          });
          const targetId = target?.dataset.libraryReorderId;
          if (targetId !== drag.hover) {
            drag.hover = targetId ?? null;
            drag.hoverSince = now;
          }
          if (target && targetId && (dropping || now - drag.hoverSince >= 90)) {
            const index = drag.order.indexOf(targetId);
            const next = drag.order.filter((value) => value !== id);
            next.splice(index, 0, id);
            if (next.join() !== drag.order.join()) {
              drag.order = next;
              drag.changed = next.join() !== drag.original.join();
              setOrder(next);
            }
          }
        }
        drag.frame = requestAnimationFrame(update);
      };
      const move = (pointer: PointerEvent) => {
        if (pointer.pointerId !== drag.pointer) return;
        pointer.preventDefault();
        drag.x = pointer.clientX;
        drag.y = pointer.clientY;
        drag.moved = true;
        if (!drag.preview) update();
      };
      const up = (pointer: PointerEvent) => {
        if (pointer.pointerId !== drag.pointer) return;
        // A release can arrive between animation frames. Apply its final
        // position before persisting, rather than saving the previous frame.
        if (drag.preview) {
          dropping = true;
          cancelAnimationFrame(drag.frame);
          drag.moved =
            drag.moved ||
            drag.x !== pointer.clientX ||
            drag.y !== pointer.clientY;
          drag.x = pointer.clientX;
          drag.y = pointer.clientY;
          update();
        }
        active.current = null;
        drag.dispose();
        if (drag.preview) settle(drag);
        else setDragging(false);
        if (drag.changed)
          save(id, drag.order).catch((failure: unknown) =>
            console.error("Library reorder failed", failure),
          );
      };
      const aborted = () => cancel();
      const lostCapture = (pointer: PointerEvent) => {
        if (pointer.pointerId === drag.pointer && active.current === drag)
          cancel();
      };
      document.addEventListener("pointermove", move, { passive: false });
      document.addEventListener("pointerup", up);
      document.addEventListener("pointercancel", aborted);
      content.addEventListener("lostpointercapture", lostCapture);
      window.addEventListener("blur", aborted);
      drag.dispose = () => {
        cancelAnimationFrame(drag.frame);
        document.removeEventListener("pointermove", move);
        document.removeEventListener("pointerup", up);
        document.removeEventListener("pointercancel", aborted);
        content.removeEventListener("lostpointercapture", lostCapture);
        window.removeEventListener("blur", aborted);
        if (content.hasPointerCapture(drag.pointer))
          content.releasePointerCapture(drag.pointer);
      };
    },
  );

  const keyDown = useEvent(
    (event: JSX.TargetedKeyboardEvent<HTMLButtonElement>, id: string) => {
      if (
        !enabled ||
        pending.current ||
        settling.current ||
        !event.altKey ||
        !["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)
      )
        return;
      event.preventDefault();
      const ids = ordered.map((item) => item.media_id);
      const from = ids.indexOf(id);
      const to = from + (["ArrowLeft", "ArrowUp"].includes(event.key) ? -1 : 1);
      if (from < 0 || to < 0 || to >= ids.length) return;
      ids.splice(from, 1);
      ids.splice(to, 0, id);
      setOrder(ids);
      save(id, ids, event.currentTarget).catch((failure: unknown) =>
        console.error("Library reorder failed", failure),
      );
    },
  );
  return {
    items: ordered,
    saving,
    dragging,
    error,
    pointerDown,
    keyDown,
    cancel,
  };
}
