import { useEffect, useRef, useState } from "preact/hooks";
import autoAnimate from "@formkit/auto-animate";
import { Avatar } from "./Avatar.tsx";
import type { Snapshot } from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { uploadFile } from "../api/media.ts";
import {
  AVATAR_SIZE,
  cropSquare,
  encodeAvatar,
} from "../features/preferences/avatar.ts";

export function AvatarSettings({ state }: { state: Snapshot }) {
  const [menu, setMenu] = useState(false);
  const [urlMode, setUrlMode] = useState(false);
  const [url, setUrl] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [zoom, setZoom] = useState(1);
  const [x, setX] = useState(50);
  const [y, setY] = useState(50);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState("");
  const animatedSettings = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const controller = useRef(new AbortController());
  useEffect(() => {
    if (!animatedSettings.current) return;
    const animation = autoAnimate(animatedSettings.current, {
      duration: 120,
      easing: "ease-out",
    });
    return () => animation.destroy?.();
  }, []);
  useEffect(() => () => controller.current.abort(), []);
  useEffect(() => {
    if (!file) return;
    const source = URL.createObjectURL(file);
    const decoded = new Image();
    decoded.onload = () => {
      if (decoded.naturalWidth * decoded.naturalHeight > 40_000_000) {
        setFeedback("Choose an image smaller than 40 megapixels.");
        setFile(null);
        return;
      }
      setImage(decoded);
      setZoom(1);
      setX(50);
      setY(50);
    };
    decoded.onerror = () => {
      setFeedback("Unable to read this image.");
      setFile(null);
    };
    decoded.src = source;
    return () => {
      decoded.onload = null;
      decoded.onerror = null;
      decoded.src = "";
      URL.revokeObjectURL(source);
    };
  }, [file]);
  useEffect(() => {
    const context = canvas.current?.getContext("2d");
    if (!image || !context) return;
    const crop = cropSquare(
      image.naturalWidth,
      image.naturalHeight,
      zoom,
      x,
      y,
    );
    context.clearRect(0, 0, AVATAR_SIZE, AVATAR_SIZE);
    context.drawImage(
      image,
      crop.x,
      crop.y,
      crop.size,
      crop.size,
      0,
      0,
      AVATAR_SIZE,
      AVATAR_SIZE,
    );
  }, [image, zoom, x, y]);
  function run(work: () => Promise<unknown>) {
    setBusy(true);
    setFeedback("");
    work()
      .then(() => {
        setFeedback("Avatar updated.");
        setMenu(false);
        setUrlMode(false);
        setFile(null);
        setImage(null);
      })
      .catch((error: unknown) => setFeedback(errorMessage(error)))
      .finally(() => setBusy(false));
  }
  async function upload() {
    const element = canvas.current;
    if (!element || !image) throw new Error("Choose an image first.");
    const blob = await encodeAvatar(element);
    const signal = controller.current.signal;
    const ticket = await api.allocateAvatar(blob.size, signal);
    try {
      await uploadFile(
        ticket,
        new File(
          [blob],
          blob.type === "image/webp" ? "avatar.webp" : "avatar.png",
          {
            type: blob.type,
          },
        ),
        signal,
        () => {},
      );
      await api.completeAvatar(ticket.media_id, signal);
    } catch (error) {
      await api.abandonMedia(ticket.media_id).catch(() => {});
      throw error;
    }
  }
  return (
    <div class="avatar-settings" ref={animatedSettings}>
      <div class="account-card">
        <button
          type="button"
          class="account-avatar"
          aria-label="Change avatar"
          aria-expanded={menu}
          disabled={busy}
          onClick={() => setMenu(!menu)}
        >
          <Avatar
            url={state.avatars?.[state.username]}
            name={state.username}
            large
            fallback
          />
        </button>
        <div>
          <strong>{state.username}</strong>
          <p>Groups: {state.groups.join(", ") || "none"}</p>
        </div>
      </div>
      {menu && (
        <div class="avatar-menu" aria-label="Avatar actions">
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setUrlMode(false);
              input.current?.click();
            }}
          >
            Upload
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setFile(null);
              setImage(null);
              setUrlMode(true);
            }}
          >
            Set URL
          </button>
          <button
            type="button"
            disabled={busy || !state.avatars?.[state.username]}
            onClick={() => run(() => api.setAvatar(null))}
          >
            Remove
          </button>
        </div>
      )}
      <input
        ref={input}
        type="file"
        accept="image/png,image/jpeg,image/webp"
        hidden
        onChange={(event) => {
          const selected = event.currentTarget.files?.[0];
          event.currentTarget.value = "";
          if (!selected) return;
          setFeedback("");
          if (
            !["image/png", "image/jpeg", "image/webp"].includes(
              selected.type,
            ) ||
            selected.size > 20 * 1024 * 1024
          ) {
            setFeedback("Choose a PNG, JPEG or WebP image up to 20 MiB.");
            return;
          }
          setImage(null);
          setFile(selected);
        }}
      />
      {urlMode && (
        <form
          class="account-form"
          onSubmit={(event) => {
            event.preventDefault();
            run(() => api.setAvatar(url.trim()));
          }}
        >
          <label for="avatar-url">Avatar image URL</label>
          <p class="account-summary">
            External images load at their original size. Upload an image to
            minimize bandwidth.
          </p>
          <input
            id="avatar-url"
            type="url"
            placeholder="https://…"
            required
            value={url}
            onInput={(event) => setUrl(event.currentTarget.value)}
            disabled={busy}
          />
          <button type="submit" disabled={busy}>
            Set avatar
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => setUrlMode(false)}
          >
            Cancel
          </button>
        </form>
      )}
      {file && (
        <div class="avatar-crop">
          <p>
            Position and zoom your image. The circle shows what everyone will
            see.
          </p>
          <canvas
            ref={canvas}
            width={AVATAR_SIZE}
            height={AVATAR_SIZE}
            aria-label="Circular avatar crop preview"
          />
          {(
            [
              ["Zoom", zoom, 1, 3, 0.01, setZoom],
              ["Horizontal position", x, 0, 100, 1, setX],
              ["Vertical position", y, 0, 100, 1, setY],
            ] as const
          ).map(([label, value, min, max, step, set]) => (
            <label key={label}>
              {label}
              <input
                type="range"
                aria-label={label}
                min={min}
                max={max}
                step={step}
                value={value}
                disabled={busy || !image}
                onInput={(event) => set(Number(event.currentTarget.value))}
              />
            </label>
          ))}
          <button
            type="button"
            disabled={busy || !image}
            onClick={() => run(upload)}
          >
            {busy ? "Uploading…" : "Confirm and upload"}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setFile(null);
              setImage(null);
            }}
          >
            Cancel
          </button>
        </div>
      )}
      {feedback && (
        <p class="account-summary" role="status">
          {feedback}
        </p>
      )}
    </div>
  );
}
