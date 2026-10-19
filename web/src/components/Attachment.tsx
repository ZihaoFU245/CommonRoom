import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { attachmentKind, documentSource, previewLimit } from "../api/media.ts";
import type { CompletedMedia, AttachmentKind } from "../api/media.ts";

export function Attachment({
  id,
  href,
  kind,
  children,
  renderHtml,
}: {
  id: string;
  href: string;
  kind: AttachmentKind;
  children: ComponentChildren;
  renderHtml?: (source: string) => ComponentChildren;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [source, setSource] = useState<string | null>(null);
  const [playback, setPlayback] = useState<{
    media: CompletedMedia;
    version: number;
    position: number;
    resume: boolean;
  } | null>(null);
  const request = useRef<AbortController | null>(null);
  const player = useRef<HTMLMediaElement | null>(null);
  const failures = useRef(0);
  const position = useRef(0);
  const playing = useRef(false);
  useEffect(
    () => () => {
      request.current?.abort();
    },
    [],
  );
  async function open() {
    if (busy) return;
    const controller = new AbortController();
    request.current?.abort();
    request.current = controller;
    setBusy(true);
    setError("");
    try {
      const media = await api.mediaInfo(id, controller.signal);
      const verifiedKind = attachmentKind(media.content_type);
      if (media.media_id !== id || verifiedKind !== kind)
        throw new Error(
          "This file cannot be previewed. Use the download link.",
        );
      if (kind === "audio" || kind === "video") {
        const probe = document.createElement(kind);
        if (!probe.canPlayType(media.content_type))
          throw new Error(
            "Your browser cannot play this format. Download the file instead.",
          );
        failures.current = 0;
        position.current = 0;
        playing.current = true;
        setPlayback((previous) => ({
          media,
          version: (previous?.version ?? -1) + 1,
          position: 0,
          resume: true,
        }));
      } else {
        if (media.size > previewLimit)
          throw new Error(
            "Preview is limited to 1 MiB. Download this file instead.",
          );
        setSource(await documentSource(media.path, controller.signal));
      }
    } catch (failure) {
      if (!controller.signal.aborted)
        setError(
          failure instanceof Error
            ? failure.message
            : "Unable to open attachment.",
        );
    } finally {
      if (!controller.signal.aborted) setBusy(false);
    }
  }
  useEffect(() => {
    if (playback?.resume)
      player.current?.play().catch(() => {
        /* Native Play remains available. */
      });
  }, [playback]);
  function playbackError() {
    const element = player.current;
    if (!playback || !element) return;
    // Refresh through the authenticated chat endpoint. Native players cannot
    // renew an expired S3 redirect themselves. Never loop on unsupported codecs.
    if (element.error?.code !== 3 && failures.current < 1) {
      failures.current++;
      setPlayback({
        ...playback,
        version: playback.version + 1,
        position: position.current,
        resume: playing.current,
      });
    } else
      setError(
        "Playback failed or this codec is unsupported. Retry or download the file.",
      );
  }
  function loaded() {
    const element = player.current;
    if (!element || !playback) return;
    if (playback.position > 0) element.currentTime = playback.position;
  }
  const mediaProps = playback
    ? {
        key: playback.version,
        ref: (element: HTMLMediaElement | null) => {
          player.current = element;
        },
        src: `${playback.media.path}?playback=${playback.version}`,
        controls: true,
        preload: "none" as const,
        onLoadedMetadata: loaded,
        onError: playbackError,
        onTimeUpdate: () => {
          if (player.current) position.current = player.current.currentTime;
        },
        onSeeking: () => {
          if (player.current) position.current = player.current.currentTime;
        },
        onPlaying: () => {
          failures.current = 0;
          playing.current = true;
        },
        onPause: () => {
          if (!player.current?.error) playing.current = false;
        },
      }
    : null;
  return (
    <span class="attachment-viewer">
      <span class="attachment-actions">
        {((!playback && source === null) || error) && (
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              open().catch(() => undefined);
            }}
          >
            {busy
              ? "Loading…"
              : error
                ? "Retry"
                : kind === "audio" || kind === "video"
                  ? "Play"
                  : "Preview file"}
          </button>
        )}
      </span>
      {error && <span role="status">{error}</span>}
      {mediaProps &&
        (kind === "video" ? (
          <video {...mediaProps} playsInline />
        ) : (
          <audio {...mediaProps} />
        ))}
      {source !== null && renderHtml?.(source)}
      <a
        class="attachment-download"
        href={`${href}?download=true`}
        target="_blank"
        rel="noopener noreferrer"
      >
        {children}
      </a>
    </span>
  );
}
