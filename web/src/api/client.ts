import { isSnapshot, isHistory, isOk, responseError } from "./protocol.ts";
import type { ClientFrame } from "./protocol.ts";

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
    ...(signal === undefined ? {} : { signal }),
    ...(body === undefined
      ? {}
      : {
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }),
  });
  const value: unknown = await response.json();
  if (!response.ok) throw new Error(responseError(value));
  if (!validate(value))
    throw new Error(
      "Invalid server response. Please reload or check the server version.",
    );
  return value;
}

export const api = {
  me: () => request("me", isSnapshot),
  login: (username: string, password: string) =>
    request("login", isOk, { username, password }),
  logout: () => request("logout", isOk, {}),
  history: (view: string, signal: AbortSignal) =>
    request(
      `history?view=${encodeURIComponent(view)}`,
      isHistory,
      undefined,
      signal,
    ),
  read: (view: string, through: number) =>
    request("read", isOk, { view, through }),
};
