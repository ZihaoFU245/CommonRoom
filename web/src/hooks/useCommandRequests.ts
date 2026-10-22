import type { RefObject } from "preact";
import { useRef, useState } from "preact/hooks";
import type { Acknowledgement } from "../api/protocol.ts";
import { sendFrame } from "../api/client.ts";
import { createRequestLifecycle } from "../features/conversation/requestLifecycle.ts";

export function useCommandRequests(socket: RefObject<WebSocket | null>) {
  const [pending, setPending] = useState(false);
  const lifecycleRef = useRef<ReturnType<typeof createRequestLifecycle> | null>(
    null,
  );
  if (!lifecycleRef.current)
    lifecycleRef.current = createRequestLifecycle((frame) => {
      const connection = socket.current;
      if (connection?.readyState !== WebSocket.OPEN)
        throw new Error("Disconnected. Wait for the connection to recover.");
      sendFrame(connection, frame);
    }, setPending);
  const lifecycle = lifecycleRef.current;
  function accountCommand(text: string): Promise<Acknowledgement> {
    if (lifecycle.pending)
      return Promise.reject(new Error("Another command is pending."));
    return new Promise((resolve) => {
      lifecycle.submit({
        command: { text, room: "@command", key: null, action: true },
        frame: { room: null, text },
        complete: resolve,
      });
    });
  }
  return { lifecycle, pending, accountCommand };
}
