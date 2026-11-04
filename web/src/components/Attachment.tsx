import { HtmlCover } from "./HtmlCover.tsx";
import { MediaDownload } from "./MediaDownload.tsx";
import { useFileUrl } from "../hooks/useFileUrl.ts";
import { HtmlSource } from "./HtmlSource.tsx";
import { lazy, Suspense } from "preact/compat";
import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { attachmentKind, documentSource, previewLimit } from "../api/media.ts";
import type { AttachmentKind } from "../api/media.ts";

const HtmlPreview = lazy(() =>
  import("./HtmlMarkdownGate.tsx")
    .then((module) => ({ default: module.HtmlPreview }))
    .catch(() => ({
      default: () => (
        <p role="status">Unable to load preview. Reload to retry.</p>
      ),
    })),
);
export interface AttachmentContext {
  messageKey: string;
  consentText: string;
  allowHtml: boolean;
}
export function Attachment({
  id,
  href,
  kind,
  children,
  context,
}: {
  id: string;
  href: string;
  kind: AttachmentKind;
  children: ComponentChildren;
  context?: AttachmentContext;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [source, setSource] = useState<string | null>(null);
  const [playback, setPlayback] = useState({
    version: 0,
    position: 0,
    resume: false,
  });
  const file = useFileUrl(href, { version: playback.version });
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
      if (media.size > previewLimit)
        throw new Error(
          "Preview is limited to 1 MiB. Download this file instead.",
        );
      if (!file.url) throw new Error("File is still loading. Try again.");
      setSource(await documentSource(href, controller.signal, file.url));
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
  function preview() {
    open().catch((failure: unknown) => {
      console.error("Attachment preview failed", failure);
    });
  }
  useEffect(() => {
    if (playback.resume)
      player.current?.play().catch(() => {
        /* Native Play remains available. */
      });
  }, [playback, file.url]);
  function playbackError() {
    const element = player.current;
    if (!element) return;
    // Retry through the worker's authorized cache pipeline after a transient
    // failure. Never loop on unsupported codecs.
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
    if (!element) return;
    if (playback.position > 0) element.currentTime = playback.position;
  }
  const mediaProps = {
    key: playback.version,
    ref: (element: HTMLMediaElement | null) => {
      player.current = element;
    },
    // The authenticated file endpoint supplies verified MIME and live access
    // checks. Native controls can wait for Play without a metadata request.
    src: file.url,
    ...(file.crossOrigin ? { crossOrigin: file.crossOrigin } : {}),
    controls: true,
    preload: "none" as const,
    onLoadedMetadata: loaded,
    onError: playbackError,
    onPlay: () => {
      playing.current = true;
    },
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
  };
  const isDocument = kind === "html" || kind === "svg";
  const status = error ? (
    <p role="status">
      {error}{" "}
      <button type="button" onClick={preview}>
        Retry
      </button>
    </p>
  ) : (
    <p role="status">Loading file…</p>
  );
  return (
    <span class="attachment-viewer">
      {!isDocument && (error || file.error) && (
        <span class="attachment-actions">
          <button
            type="button"
            onClick={() => {
              setError("");
              failures.current = 0;
              setPlayback({
                version: playback.version + 1,
                position: position.current,
                resume: true,
              });
            }}
          >
            Retry
          </button>
        </span>
      )}
      {!isDocument && (error || file.error) && (
        <span role="status">{error || file.error}</span>
      )}
      {!isDocument && !file.url && !file.error && (
        <span role="status">Loading file…</span>
      )}
      {!isDocument &&
        file.url &&
        (kind === "video" ? (
          <video {...mediaProps} playsInline />
        ) : (
          <audio {...mediaProps} />
        ))}
      {isDocument && (
        <HtmlCover
          text={source ?? ""}
          label={children}
          messageKey={context?.messageKey ?? id}
          choiceKey={JSON.stringify([
            context?.messageKey ?? id,
            "attachment",
            id,
          ])}
          consentText={context?.consentText ?? id}
          allowRender={context?.allowHtml ?? false}
          textStatus={source === null ? status : undefined}
          onModeChange={(mode) => {
            if (mode && source === null) preview();
          }}
        >
          {source === null ? (
            status
          ) : (
            <Suspense fallback={<p role="status">Loading preview…</p>}>
              <HtmlPreview
                text={source}
                fallback={<HtmlSource text={source} />}
              />
            </Suspense>
          )}
        </HtmlCover>
      )}
      <MediaDownload
        className="attachment-download"
        href={`${href}?download=true`}
      >
        {isDocument && "Download "}
        {children}
      </MediaDownload>
    </span>
  );
}
