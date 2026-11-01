import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { errorMessage } from "../api/protocol.ts";

/** A sent sticker; its context menu copies it into the viewer's own library. */
export function StickerImage({ id, src }: { id: string; src: string }) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [status, setStatus] = useState("");
  const panel = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => () => clearTimeout(timer.current), []);
  useEffect(() => {
    if (!menu) return;
    function dismiss(event: Event) {
      if (
        event.type === "keydown" &&
        event instanceof KeyboardEvent &&
        event.key !== "Escape"
      )
        return;
      if (event.target instanceof Node && panel.current?.contains(event.target))
        return;
      setMenu(null);
    }
    document.addEventListener("pointerdown", dismiss);
    document.addEventListener("keydown", dismiss);
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("blur", dismiss);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      document.removeEventListener("keydown", dismiss);
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("blur", dismiss);
    };
  }, [menu]);
  async function collect() {
    setStatus("Adding…");
    try {
      await api.collectSticker(id);
      setStatus("Added to your stickers");
    } catch (failure) {
      setStatus(errorMessage(failure));
    }
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setMenu(null), 1500);
  }
  return (
    <>
      <img
        class="sticker"
        src={src}
        alt="Sticker"
        loading="lazy"
        decoding="async"
        draggable={false}
        onContextMenu={(event) => {
          event.preventDefault();
          clearTimeout(timer.current);
          setStatus("");
          setMenu({ x: event.clientX, y: event.clientY });
        }}
      />
      {menu && (
        <div
          class="sticker-menu"
          ref={panel}
          role="menu"
          style={{
            left: `${Math.min(menu.x, window.innerWidth - 220)}px`,
            top: `${Math.min(menu.y, window.innerHeight - 60)}px`,
          }}
        >
          {status ? (
            <span role="status">{status}</span>
          ) : (
            <button
              type="button"
              role="menuitem"
              autoFocus
              onClick={() => {
                collect().catch(() => undefined);
              }}
            >
              Add to my stickers
            </button>
          )}
        </div>
      )}
    </>
  );
}
