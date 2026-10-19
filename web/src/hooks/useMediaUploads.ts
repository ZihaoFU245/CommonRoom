import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { api } from "../api/client.ts";
import { uploadFile } from "../api/media.ts";
import type { CompletedMedia } from "../api/media.ts";
import { errorMessage } from "../api/protocol.ts";

export interface Attachment {
  media: CompletedMedia;
  preview: string | null;
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
    if (previous?.preview) URL.revokeObjectURL(previous.preview);
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
        preview: media.content_type.startsWith("image/")
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
    cancel,
    published,
    retry: retryFile ? () => start(retryFile) : undefined,
  };
}
