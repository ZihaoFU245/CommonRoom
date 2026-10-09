import { useEffect, useRef, useState } from "preact/hooks";
import type { Snapshot, Acknowledgement } from "../api/protocol.ts";
import { parseFrame, errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { snapshotSync } from "./sync.ts";
interface Callbacks {
  onSnapshot: (previous: Snapshot, data: Snapshot, baseline: boolean) => void;
  onResult: (data: Acknowledgement) => void;
  onDisconnect: () => void;
  onLogout: () => void;
}
export function useConnection(initial: Snapshot, handlers: Callbacks) {
  const [state, setState] = useState(initial);
  const currentState = useRef(initial);
  const [status, setStatus] = useState<"connecting" | "online" | "offline">(
    "connecting",
  );
  const socket = useRef<WebSocket | null>(null);
  const callbacks = useRef(handlers);
  callbacks.current = handlers;
  useEffect(() => {
    let stopped = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let attempts = 0;
    const sync = snapshotSync(
      api.me,
      (data, baseline) => {
        const previous = currentState.current;
        currentState.current = data;
        setState(data);
        callbacks.current.onSnapshot(previous, data, baseline);
      },
      (error) => {
        if (
          ["Please log in.", "Account unavailable."].includes(
            errorMessage(error),
          )
        )
          callbacks.current.onLogout();
      },
    );
    const resume = () => {
      if (!document.hidden) sync.refresh();
    };
    document.addEventListener("visibilitychange", resume);
    window.addEventListener("focus", resume);
    window.addEventListener("pageshow", resume);
    window.addEventListener("online", resume);
    async function connect() {
      if (stopped) return;
      setStatus("connecting");
      const url = new URL("ws", document.baseURI);
      url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
      const ws = new WebSocket(url);
      socket.current = ws;
      ws.onopen = () => {
        if (stopped || socket.current !== ws) return;
        attempts = 0;
        setStatus("online");
        sync.refresh();
      };
      let baseline = true;
      ws.onmessage = (event) => {
        if (stopped || socket.current !== ws) return;
        const data = parseFrame(String(event.data));
        if (!data) return;
        if (data.kind === "snapshot") {
          sync.receive(data, baseline);
          baseline = false;
        } else if (data.kind === "read") {
          sync.receive({ ...currentState.current, unread: data.unread }, true);
        } else if (data.kind === "notice" || data.kind === "error") {
          callbacks.current.onResult(data);
        }
      };
      ws.onclose = async () => {
        if (stopped) return;
        setStatus("offline");
        callbacks.current.onDisconnect();
        await sync.refresh();
        if (!stopped)
          retry = setTimeout(connect, Math.min(1000 * 2 ** attempts++, 15000));
      };
      ws.onerror = () => ws.close();
    }
    connect();
    return () => {
      stopped = true;
      sync.stop();
      document.removeEventListener("visibilitychange", resume);
      window.removeEventListener("focus", resume);
      window.removeEventListener("pageshow", resume);
      window.removeEventListener("online", resume);
      clearTimeout(retry);
      socket.current?.close();
    };
  }, []);

  return { state, currentState, status, socket };
}
