import { isUploadTicket, isCompletedMedia, isMediaConfig } from "./media.ts";
import {
  isSnapshot,
  isHistory,
  isOk,
  responseError,
  isBotCommandDirectory,
} from "./protocol.ts";
import type { ClientFrame, HistoryRange } from "./protocol.ts";

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
  socket.send(JSON.stringify(frame));
}

async function request<T>(
  path: string,
  validate: (value: unknown) => value is T,
  body?: unknown,
  signal?: AbortSignal,
  timeoutMs = 15000,
): Promise<T> {
  const response = await fetch(new URL(`api/${path}`, document.baseURI), {
    method: body === undefined ? "GET" : "POST",
    credentials: "same-origin",
    cache: "no-store",
    signal: signal
      ? AbortSignal.any([signal, AbortSignal.timeout(timeoutMs)])
      : AbortSignal.timeout(timeoutMs),
    ...(body === undefined
      ? {}
      : {
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }),
  });
  const value: unknown = await response.json().catch(() => null);
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
}

export const api = {
  mediaInfo: (id: string, signal: AbortSignal) =>
    request(
      `media/${encodeURIComponent(id)}/info`,
      isCompletedMedia,
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
  ) =>
    request(
      "media/uploads",
      isUploadTicket,
      { conversation_id: conversationId, filename, size },
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
  botCommands: (conversationId: string, signal: AbortSignal) =>
    request(
      `bot-commands?${new URLSearchParams({ conversation_id: conversationId })}`,
      isBotCommandDirectory,
      undefined,
      signal,
    ),
  me: () => request("me", isSnapshot),
  session: () => request("session", isOk),
  login: (username: string, password: string) =>
    request("login", isOk, { username, password }),
  logout: () => request("logout", isOk, {}),
  history: (view: string, signal: AbortSignal, range: HistoryRange = {}) => {
    const query = new URLSearchParams({ view });
    for (const [key, value] of Object.entries(range))
      query.set(key, String(value));
    return request(`history?${query.toString()}`, isHistory, undefined, signal);
  },
  read: (view: string, through: number) =>
    request("read", isOk, { view, through }),
};
