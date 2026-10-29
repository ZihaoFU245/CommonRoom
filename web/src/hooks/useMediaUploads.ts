import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { filenameHint, uploadFile } from "../api/media.ts";
import type { CompletedMedia } from "../api/media.ts";
import type { Sticker } from "../api/stickers.ts";
import { errorMessage } from "../api/protocol.ts";

export interface Attachment {
  media: CompletedMedia;
  preview: string | null;
  /** Rendered as a small sticker; the preview is a server URL, not a blob. */
  sticker?: boolean;
}
export function useMediaUploads(
  conversationId: string | undefined,
  serverInstance?: string,
) {
  const [enabled, setEnabled] = useState(false);
  const [maxSize, setMaxSize] = useState(0);
  const [status, setStatus] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [retryFile, setRetryFile] = useState<File | null>(null);
  const [attachment, setAttachment] = useState<Attachment | null>(null);
  const currentAttachment = useRef<Attachment | null>(null);
  const controller = useRef<AbortController | null>(null);
  const allocated = useRef<string | null>(null);
  const previousInstance = useRef(serverInstance);
  useEffect(() => {
    if (!serverInstance) return;
    const previous = previousInstance.current;
    previousInstance.current = serverInstance;
    if (!previous || previous === serverInstance) return;
    if (currentAttachment.current || controller.current) {
      controller.current?.abort();
      controller.current = null;
      allocated.current = null;
      release();
      setBusy(false);
      setStatus("");
      setError("The server restarted. Attach the file again before sending.");
    }
  }, [serverInstance]);
  const currentConversation = useRef(conversationId);
  currentConversation.current = conversationId;
  function abandon(id: string | null) {
    if (id)
      api.abandonMedia(id).catch(() => {
        /* Durable cleanup handles offline removal. */
      });
  }
  function release() {
    const previous = currentAttachment.current;
    if (previous?.preview && !previous.sticker)
      URL.revokeObjectURL(previous.preview);
    currentAttachment.current = null;
    setAttachment(null);
  }
  useEffect(() => {
    const request = new AbortController();
    api
      .mediaConfig(request.signal)
      .then((config) => {
        if (!request.signal.aborted) {
          setEnabled(config.enabled);
          setMaxSize(config.max_size);
        }
      })
      .catch(() => {
        /* Hide upload controls when configuration is unavailable. */
      });
    return () => request.abort();
  }, [serverInstance]);
  useLayoutEffect(() => {
    setStatus("");
    setError("");
    setBusy(false);
    setRetryFile(null);
    return () => {
      controller.current?.abort();
      controller.current = null;
      abandon(allocated.current);
      allocated.current = null;
      release();
    };
  }, [conversationId]);
  async function start(file: File) {
    if (controller.current || currentAttachment.current) return;
    if (!conversationId) {
      setError("Select a conversation before attaching files.");
      return;
    }
    if (!file.size || file.size > maxSize) {
      setError(`Choose a file between 1 byte and ${maxSize} bytes.`);
      return;
    }
    const active = new AbortController();
    controller.current = active;
    const valid = () =>
      !active.signal.aborted && currentConversation.current === conversationId;
    let uploadId: string | null = null;
    setBusy(true);
    setError("");
    setRetryFile(null);
    setStatus("Preparing upload…");
    try {
      const ticket = await api.allocateMedia(
        conversationId,
        file.name,
        file.size,
        active.signal,
        {
          filename: filenameHint(file.name),
          ...(file.type ? { "content-type": file.type } : {}),
          client: "commonroom-web",
        },
      );
      if (!valid()) {
        abandon(ticket.media_id);
        return;
      }
      uploadId = ticket.media_id;
      allocated.current = ticket.media_id;
      setStatus("Uploading 0%…");
      await uploadFile(ticket, file, active.signal, (percent) => {
        if (valid()) setStatus(`Uploading ${percent}%…`);
      });
      if (!valid()) return;
      setStatus("Verifying upload…");
      const media = await api.completeMedia(ticket.media_id, active.signal);
      if (!valid()) return;
      const value = {
        media,
        preview:
          file.size <= 4 * 1024 * 1024 &&
          media.content_type.startsWith("image/") &&
          media.content_type !== "image/svg+xml"
            ? URL.createObjectURL(file)
            : null,
      };
      currentAttachment.current = value;
      setAttachment(value);
      setStatus("Attachment ready.");
    } catch (failure) {
      if (valid()) {
        setError(errorMessage(failure));
        setRetryFile(file);
        setStatus("");
      }
      if (uploadId && allocated.current === uploadId) {
        abandon(uploadId);
        allocated.current = null;
      }
    } finally {
      if (controller.current === active) controller.current = null;
      if (valid()) setBusy(false);
    }
  }
  /** Copies a library sticker into this conversation as the attachment. */
  async function attachSticker(sticker: Sticker) {
    if (controller.current || currentAttachment.current) return;
    if (!conversationId) {
      setError("Select a conversation before adding a sticker.");
      return;
    }
    const active = new AbortController();
    controller.current = active;
    const valid = () =>
      !active.signal.aborted && currentConversation.current === conversationId;
    setBusy(true);
    setError("");
    setRetryFile(null);
    setStatus("Adding sticker…");
    try {
      const media = await api.sendSticker(
        sticker.id,
        conversationId,
        active.signal,
      );
      if (!valid()) return;
      const value = { media, preview: sticker.path, sticker: true };
      allocated.current = media.media_id;
      currentAttachment.current = value;
      setAttachment(value);
      setStatus("");
    } catch (failure) {
      if (valid()) {
        setError(errorMessage(failure));
        setStatus("");
      }
    } finally {
      if (controller.current === active) controller.current = null;
      if (valid()) setBusy(false);
    }
  }
  function cancel() {
    controller.current?.abort();
    controller.current = null;
    abandon(allocated.current);
    allocated.current = null;
    release();
    setBusy(false);
    setStatus("");
    setError("");
    setRetryFile(null);
  }
  function published(id: string) {
    if (currentAttachment.current?.media.media_id !== id) return;
    allocated.current = null;
    release();
    setStatus("");
  }
  return {
    enabled,
    maxSize,
    busy,
    status,
    error,
    attachment,
    start,
    attachSticker,
    cancel,
    published,
    retry: retryFile ? () => start(retryFile) : undefined,
  };
}
