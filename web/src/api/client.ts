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
): Promise<T> {
  const response = await fetch(new URL(`api/${path}`, document.baseURI), {
    method: body === undefined ? "GET" : "POST",
    credentials: "same-origin",
    cache: "no-store",
    signal: signal
      ? AbortSignal.any([signal, AbortSignal.timeout(15000)])
      : AbortSignal.timeout(15000),
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
