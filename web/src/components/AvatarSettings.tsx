import { controlMotion } from "../features/preferences/motion.ts";
import { useEffect, useRef, useState } from "preact/hooks";
import autoAnimate from "@formkit/auto-animate";
import { Avatar } from "./Avatar.tsx";
import type { Snapshot } from "../api/protocol.ts";
import { saveErrorMessage } from "../features/preferences/saveFeedback.ts";
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
  const [image, setImage] = useState<HTMLCanvasElement | null>(null);
  const [zoom, setZoom] = useState(1);
  const [x, setX] = useState(50);
  const [y, setY] = useState(50);
  const [busy, setBusy] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [feedback, setFeedback] = useState("");
  const [failed, setFailed] = useState(false);
  const [preview, setPreview] = useState<string>();
  const [avatarVersion, setAvatarVersion] = useState(0);
  const [committed, setCommitted] = useState<{
    path: string | undefined;
    previous: string | undefined;
  }>();
  const avatarButton = useRef<HTMLButtonElement>(null);
  const currentAvatar = state.avatars?.[state.username];
  useEffect(() => {
    if (committed && currentAvatar !== committed.previous)
      setCommitted(undefined);
  }, [currentAvatar, committed]);
  const displayed =
    committed && currentAvatar === committed.previous
      ? committed.path
      : currentAvatar;
  useEffect(
    () => () => {
      if (preview) URL.revokeObjectURL(preview);
    },
    [preview],
  );
  const animatedSettings = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const controller = useRef(new AbortController());
  useEffect(() => {
    if (!animatedSettings.current) return;
    const animation = autoAnimate(animatedSettings.current, controlMotion);
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
      const working = document.createElement("canvas");
      const scale = Math.min(
        1,
        1024 / Math.max(decoded.naturalWidth, decoded.naturalHeight),
      );
      working.width = Math.max(1, Math.round(decoded.naturalWidth * scale));
      working.height = Math.max(1, Math.round(decoded.naturalHeight * scale));
      const context = working.getContext("2d");
      if (!context) {
        setFeedback("Unable to resize image.");
        setFile(null);
        return;
      }
      context.drawImage(decoded, 0, 0, working.width, working.height);
      decoded.onload = null;
      decoded.onerror = null;
      decoded.src = "";
      URL.revokeObjectURL(source);
      setImage(working);
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
    const frame = requestAnimationFrame(() => {
      const crop = cropSquare(image.width, image.height, zoom, x, y);
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
    });
    return () => cancelAnimationFrame(frame);
  }, [image, zoom, x, y]);
  function run(work: () => Promise<unknown>) {
    if (busy) return;
    setBusy(true);
    setFailed(false);
    setFeedback("Updating avatar…");
    work()
      .then(() => {
        setFeedback("Avatar updated.");
      })
      .catch((error: unknown) => {
        setPreview(undefined);
        setFailed(true);
        setFeedback(saveErrorMessage(error));
      })
      .finally(() => {
        setBusy(false);
        setUploading(false);
        avatarButton.current?.focus();
      });
  }
  async function upload() {
    setUploading(true);
    const signal = controller.current.signal;
    const element = canvas.current;
    if (!element || !image) throw new Error("Choose an image first.");
    // Draw synchronously too: confirmation may precede the pending preview frame.
    const crop = cropSquare(image.width, image.height, zoom, x, y);
    const context = element.getContext("2d");
    if (!context) throw new Error("Unable to crop image.");
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
    const blob = await encodeAvatar(element);
    signal.throwIfAborted();
    setPreview(URL.createObjectURL(blob));
    setMenu(false);
    avatarButton.current?.focus();
    const uploadId = crypto.randomUUID();
    let allocated = false;
    try {
      const ticket = await api.allocateAvatar(blob.size, signal, uploadId);
      allocated = true;
      await uploadFile(
        ticket,
        new File(
          [blob],
          blob.type === "image/webp" ? "avatar.webp" : "avatar.png",
          { type: blob.type },
        ),
        signal,
      );
      const complete = await api.completeAvatar(ticket.media_id, signal);
      setCommitted({ path: complete.path, previous: currentAvatar });
      setPreview(undefined);
      setFile(null);
      setImage(null);
    } catch (error) {
      setPreview(undefined);
      // A failed optimistic frame must not survive if the previous URL also fails.
      setAvatarVersion((version) => version + 1);
      setFailed(true);
      setFeedback(
        signal.aborted ? "Upload cancelled." : saveErrorMessage(error),
      );
      // Recover a lost allocation response with the same ID before abandoning.
      // A replay cannot reserve another slot, even when the first request is late.
      if (!allocated)
        await api
          .allocateAvatar(blob.size, new AbortController().signal, uploadId)
          .catch(() => {});
      await api.abandonMedia(uploadId).catch(() => {});
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
          ref={avatarButton}
          aria-busy={busy}
          onClick={() => {
            if (!busy) setMenu(!menu);
          }}
        >
          <Avatar
            key={avatarVersion}
            url={displayed}
            {...(preview ? { preview } : {})}
            decorative={false}
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
      {feedback && (
        <p
          class={failed ? "account-error" : "account-summary"}
          role={failed ? "alert" : "status"}
        >
          {feedback}
        </p>
      )}
      {busy && uploading && (
        <button
          type="button"
          onClick={() => {
            controller.current.abort();
            controller.current = new AbortController();
          }}
        >
          Cancel upload
        </button>
      )}
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
            disabled={busy || !displayed}
            onClick={() =>
              run(async () => {
                await api.setAvatar(null);
                setCommitted({ path: undefined, previous: currentAvatar });
                setPreview(undefined);
                setFile(null);
                setImage(null);
                setMenu(false);
              })
            }
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
          setFailed(false);
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
            const value = url.trim();
            run(async () => {
              await api.setAvatar(value);
              setCommitted({ path: value, previous: currentAvatar });
              setPreview(undefined);
              setUrlMode(false);
              setMenu(false);
            });
          }}
        >
          <label for="avatar-url">Avatar image URL</label>
          <p class="account-summary">
            External hosts receive viewers’ IP addresses and may serve large
            images. Viewers must enable external avatars in Security. Upload an
            image for everyone to see it by default.
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
    </div>
  );
}
