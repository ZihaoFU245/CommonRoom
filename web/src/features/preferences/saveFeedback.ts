import { ApiError } from "../../api/client.ts";
import { errorMessage } from "../../api/protocol.ts";

/** Show actionable save feedback without exposing server/storage diagnostics. */
export function saveErrorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.status === 401) return "Your session has expired. Sign in again.";
    if (error.status >= 500)
      return "Couldn't save your changes right now. Please try again.";
  }
  if (error instanceof TypeError)
    return "Couldn't connect to the server. Check your connection and try again.";
  if (error instanceof Error && error.name === "TimeoutError")
    return "The request took too long. Please try again.";
  const message = errorMessage(error);
  // WebSocket command failures carry text rather than an HTTP status.
  return message.startsWith("Storage unavailable")
    ? "Couldn't save your changes right now. Please try again."
    : message;
}
