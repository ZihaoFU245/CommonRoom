import { mediaConfigCache } from "./mediaConfigCache.ts";
import { serverBaseUrl } from "./server.ts";
import type { PreferenceState } from "../features/preferences/account.ts";
import { isCompletedAvatar } from "../features/preferences/avatar.ts";
import { responseJson } from "./responseJson.ts";
import { requestAbort } from "./requestAbort.ts";
import { mediaFileUrl } from "./mediaFile.ts";
import { invalidateMedia } from "../features/conversation/mediaInvalidation.ts";
import { mediaStatusCache } from "../features/conversation/mediaStatusCache.ts";
import { resolveMediaRead } from "../features/conversation/mediaController.ts";
import { isLibraryPage } from "./library.ts";
import type { LibraryAction, LibraryView } from "./library.ts";
import { isUploadTicket, isCompletedMedia, isMediaConfig } from "./media.ts";
import {
  isSnapshot,
  isHistory,
  isOk,
  responseError,
  isBotCommandDirectory,
} from "./protocol.ts";
import type {
  BotCommandDirectory,
  ClientFrame,
  HistoryRange,
} from "./protocol.ts";

export class ApiError extends Error {
  readonly status: number;
  readonly code: string | undefined;
  readonly retryable: boolean | undefined;
  constructor(status: number, message: string, details?: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    const fields =
      details !== null && typeof details === "object" ? details : undefined;
    this.code =
      fields && "code" in fields && typeof fields.code === "string"
        ? fields.code
        : undefined;
    this.retryable =
      fields && "retryable" in fields && typeof fields.retryable === "boolean"
        ? fields.retryable
        : undefined;
  }
}
export function isUnauthorized(error: unknown) {
  return error instanceof ApiError && error.status === 401;
}

export function sendFrame(socket: WebSocket, frame: ClientFrame) {
  if (socket.readyState !== 1)
    throw new Error("Disconnected. Please reconnect.");
  socket.send(JSON.stringify(frame));
}

async function request<T>(
  path: string,
  validate: (value: unknown) => value is T,
  body?: unknown,
  signal?: AbortSignal,
  timeoutMs = 15000,
  nameColors = false,
): Promise<T> {
  const cancellation = requestAbort(timeoutMs, signal);
  try {
    const response = await fetch(new URL(`api/${path}`, serverBaseUrl()), {
      method: body === undefined ? "GET" : "POST",
      credentials: "include",
      // API responses are no-store; default fetch mode permits preflight reuse.
      cache: "default",
      signal: cancellation.signal,
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      ...(body !== undefined || nameColors
        ? {
            headers: {
              ...(body === undefined
                ? {}
                : { "Content-Type": "application/json" }),
              ...(nameColors ? { "X-Chat-Name-Colors": "1" } : {}),
            },
          }
        : {}),
    });
    const value = await responseJson(response);
    cancellation.signal.throwIfAborted();
    if (response.status === 401) {
      mediaConfigCache.clear();
      invalidateMedia(undefined, true).catch((error: unknown) => {
        console.error("Session media invalidation failed", error);
      });
    }
    if (!response.ok)
      throw new ApiError(
        response.status,
        value === null
          ? `Server request failed (${response.status}).`
          : responseError(value),
        value,
      );
    if (!validate(value))
      throw new Error(
        "Invalid server response. Please reload or check the server version.",
      );
    return value;
  } finally {
    cancellation.dispose();
  }
}

function isPreferences(value: unknown): value is PreferenceState {
  if (
    typeof value !== "object" ||
    value === null ||
    !("values" in value) ||
    !("sync" in value)
  )
    return false;
  const sync = value.sync;
  return (
    typeof value.values === "object" &&
    value.values !== null &&
    !Array.isArray(value.values) &&
    Object.values(value.values).every(
      (item: unknown) => typeof item === "string",
    ) &&
    typeof sync === "object" &&
    sync !== null &&
    "instance" in sync &&
    typeof sync.instance === "string" &&
    "revision" in sync &&
    typeof sync.revision === "number" &&
    Number.isSafeInteger(sync.revision) &&
    sync.revision >= 0
  );
}

export const api = {
  fileLink: async (href: string, signal: AbortSignal, download = false) => {
    const base = serverBaseUrl();
    const file = mediaFileUrl(href, base);
    if (file?.base !== base) throw new Error("Invalid managed file URL.");
    signal.throwIfAborted();
    const result = await resolveMediaRead(file.url.href, false, download, true);
    signal.throwIfAborted();
    if (!result.url || result.info.expired || result.info.unavailable)
      throw new ApiError(410, "Media unavailable.");
    return { url: result.url };
  },
  preferences: (values: Record<string, string | null>, initialize: boolean) =>
    request("preferences", isPreferences, { values, initialize }),
  setDisplayName: (value: string | null, room?: string) =>
    request("displayname", isOk, { value, room }),
  setAvatar: (url: string | null) => request("avatars", isOk, { url }),
  allocateAvatar: (size: number, signal: AbortSignal, uploadId: string) =>
    request(
      "avatars/uploads",
      isUploadTicket,
      { size, upload_id: uploadId },
      signal,
    ),
  completeAvatar: (id: string, signal: AbortSignal) =>
    request(
      `avatars/uploads/${encodeURIComponent(id)}/complete`,
      isCompletedAvatar,
      {},
      signal,
      120000,
    ),
  mediaInfo: async (id: string, signal: AbortSignal) => {
    signal.throwIfAborted();
    const value = await resolveMediaRead(
      new URL(`api/media/${encodeURIComponent(id)}/file`, serverBaseUrl()).href,
    );
    signal.throwIfAborted();
    if (value.info.expired || value.info.unavailable)
      throw new ApiError(410, "Media unavailable.");
    return value.info;
  },
  cachedMediaStatus: (id: string) =>
    mediaStatusCache.peek(`${serverBaseUrl()}${id}`),
  mediaStatus: (id: string, signal: AbortSignal, refresh = false) => {
    signal.throwIfAborted();
    return mediaStatusCache.load(
      `${serverBaseUrl()}${id}`,
      async () =>
        (
          await resolveMediaRead(
            new URL(`api/media/${encodeURIComponent(id)}/file`, serverBaseUrl())
              .href,
            refresh,
          )
        ).info,
      refresh,
    );
  },
  library: (view: LibraryView, after: string, signal: AbortSignal) =>
    request(
      `library?owned=${view === "owned"}&recent=${view === "recent"}&after=${encodeURIComponent(after)}`,
      isLibraryPage,
      undefined,
      signal,
    ),
  libraryChange: (
    id: string,
    action: LibraryAction,
    source?: string,
    confirmLast = false,
  ) =>
    request(`library/${encodeURIComponent(id)}`, isOk, {
      action,
      source_conversation: source ?? null,
      confirm_last: confirmLast,
    }).then(async (result) => {
      await invalidateMedia([id]);
      return result;
    }),
  mediaConfig: (signal: AbortSignal) =>
    mediaConfigCache.load(serverBaseUrl(), signal, (sharedSignal) =>
      request("media/config", isMediaConfig, undefined, sharedSignal),
    ),
  allocateMedia: (
    conversationId: string,
    filename: string,
    size: number,
    signal: AbortSignal,
    hints: Record<string, string> = {},
  ) =>
    request(
      "media/uploads",
      isUploadTicket,
      { conversation_id: conversationId, filename, size, hints },
      signal,
    ),
  abandonMedia: (id: string) =>
    request(`media/uploads/${encodeURIComponent(id)}/abandon`, isOk, {}),
  completeMedia: (id: string, signal: AbortSignal) =>
    request(
      `media/uploads/${encodeURIComponent(id)}/complete`,
      isCompletedMedia,
      {},
      signal,
      120000,
    ),
  botCommands: async (signal: AbortSignal): Promise<BotCommandDirectory> => {
    const conversations: BotCommandDirectory["conversations"] = [];
    let after: string | undefined;
    do {
      const page = await request(
        `bot-commands${after === undefined ? "" : `?after=${encodeURIComponent(after)}`}`,
        isBotCommandDirectory,
        undefined,
        signal,
      );
      conversations.push(...page.conversations);
      if (!page.next) return { conversations };
      if (after !== undefined && page.next <= after)
        throw new Error("Command directory cursor did not advance.");
      after = page.next;
    } while (!signal.aborted);
    signal.throwIfAborted();
    return { conversations };
  },
  me: async (nameColors = false) => {
    const base = serverBaseUrl();
    const result = await request(
      "me",
      isSnapshot,
      undefined,
      undefined,
      15000,
      nameColors,
    );
    if (serverBaseUrl() === base)
      mediaConfigCache.observeInstance(base, result.sync?.instance);
    return result;
  },
  session: () => request("session", isOk),
  login: async (username: string, password: string) => {
    mediaConfigCache.clear();
    await invalidateMedia(undefined, true);
    const result = await request("login", isOk, { username, password });
    mediaConfigCache.clear();
    await invalidateMedia(undefined, true);
    return result;
  },
  logout: async () => {
    mediaConfigCache.clear();
    await invalidateMedia(undefined, true);
    const result = await request("logout", isOk, {});
    mediaConfigCache.clear();
    await invalidateMedia(undefined, true);
    return result;
  },
  history: (
    view: string,
    signal: AbortSignal,
    range: HistoryRange = {},
    nameColors = false,
  ) => {
    const query = new URLSearchParams({ view });
    for (const [key, value] of Object.entries(range))
      query.set(key, String(value));
    return request(
      `history?${query.toString()}`,
      isHistory,
      undefined,
      signal,
      15000,
      nameColors,
    );
  },
  read: (view: string, through: number) =>
    request("read", isOk, { view, through }),
  /** `room` is a stable room ID; null sets the `@global` color. */
  color: (color: string | null, room: string | null) =>
    request("color", isOk, { color, room }),
};
