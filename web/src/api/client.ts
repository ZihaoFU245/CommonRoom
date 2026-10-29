import { isCompletedAvatar } from "../features/preferences/avatar.ts";
import { responseJson } from "./responseJson.ts";
import { requestAbort } from "./requestAbort.ts";
import {
  isUploadTicket,
  isCompletedMedia,
  isMediaConfig,
  isMediaInfo,
} from "./media.ts";
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
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
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
    const response = await fetch(new URL(`api/${path}`, document.baseURI), {
      method: body === undefined ? "GET" : "POST",
      credentials: "same-origin",
      cache: "no-store",
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
    if (!response.ok)
      throw new ApiError(
        response.status,
        value === null
          ? `Server request failed (${response.status}).`
          : responseError(value),
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

export const api = {
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
  mediaInfo: (id: string, signal: AbortSignal) =>
    request(
      `media/${encodeURIComponent(id)}/info`,
      isCompletedMedia,
      undefined,
      signal,
    ),
  mediaStatus: (id: string, signal: AbortSignal) =>
    request(
      `media/${encodeURIComponent(id)}/info?include_expired=true`,
      isMediaInfo,
      undefined,
      signal,
    ),
  mediaConfig: (signal: AbortSignal) =>
    request("media/config", isMediaConfig, undefined, signal),
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
  me: (nameColors = false) =>
    request("me", isSnapshot, undefined, undefined, 15000, nameColors),
  session: () => request("session", isOk),
  login: (username: string, password: string) =>
    request("login", isOk, { username, password }),
  logout: () => request("logout", isOk, {}),
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
