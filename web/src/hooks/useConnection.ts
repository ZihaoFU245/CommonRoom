import { useEffect, useRef, useState } from "preact/hooks";
import type {
  Snapshot,
  Acknowledgement,
  BotActivity,
} from "../api/protocol.ts";
import { applyAvatar } from "../features/preferences/avatar.ts";
import { parseFrame } from "../api/protocol.ts";
import { api, isUnauthorized } from "../api/client.ts";
import {
  boundSnapshot,
  applyConversation,
} from "../features/conversation/historyCache.ts";
import { snapshotSync, olderSnapshot } from "./sync.ts";
interface Callbacks {
  onSnapshot: (previous: Snapshot, data: Snapshot, baseline: boolean) => void;
  onResult: (data: Acknowledgement) => void;
  onDisconnect: () => void;
  onLogout: () => void;
  whenIdle?: (callback: () => void) => () => void;
}
export function useConnection(
  initial: Snapshot,
  handlers: Callbacks,
  nameColors = false,
) {
  const [state, setState] = useState(initial);
  const [botCommandVersions, setBotCommandVersions] = useState(
    new Map<string, number>(),
  );
  const [botActivities, setBotActivities] = useState<BotActivity[]>([]);
  const currentState = useRef(initial);
  const [status, setStatus] = useState<"connecting" | "online" | "offline">(
    "connecting",
  );
  const socket = useRef<WebSocket | null>(null);
  const callbacks = useRef(handlers);
  callbacks.current = handlers;
  const projection = useRef(nameColors);
  const changeProjection = useRef<(enabled: boolean) => void>(() => {});
  useEffect(() => {
    let stopped = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let resumeTimer: ReturnType<typeof setTimeout> | undefined;
    let awaitingBaseline = true;
    let attempts = 0;
    const handleSyncError = (error: unknown) => {
      if (isUnauthorized(error)) {
        stopped = true;
        clearTimeout(retry);
        clearTimeout(resumeTimer);
        socket.current?.close();
        callbacks.current.onLogout();
      }
      // Transient refresh failures retry on reconnect/focus; socket updates remain authoritative.
    };
    const createSync = () =>
      snapshotSync(
        () => api.me(projection.current),
        (data, baseline) => {
          const previous = currentState.current;
          currentState.current = data;
          setState(data);
          callbacks.current.onSnapshot(previous, data, baseline);
        },
        handleSyncError,
        olderSnapshot,
      );
    let sync = createSync();
    changeProjection.current = (enabled) => {
      if (stopped || projection.current === enabled) return;
      projection.current = enabled;
      // All acknowledgements have arrived. Replace the projection without
      // reporting a transport failure or accepting an old HTTP refresh.
      const previous = socket.current;
      socket.current = null;
      previous?.close();
      clearTimeout(retry);
      sync.stop();
      sync = createSync();
      connect();
    };
    const resume = () => {
      clearTimeout(resumeTimer);
      if (stopped || document.hidden) return;
      if (socket.current?.readyState === WebSocket.CLOSED) {
        clearTimeout(retry);
        connect();
      } else if (
        socket.current?.readyState === WebSocket.OPEN &&
        !awaitingBaseline
      ) {
        // Browsers emit focus/visibility/pageshow together. Let that burst settle;
        // an opening/reconnecting socket already supplies a complete baseline.
        resumeTimer = setTimeout(() => {
          if (
            !stopped &&
            !document.hidden &&
            !awaitingBaseline &&
            socket.current?.readyState === WebSocket.OPEN
          )
            sync.refresh().catch(handleSyncError);
        }, 100);
      }
    };
    document.addEventListener("visibilitychange", resume);
    window.addEventListener("focus", resume);
    window.addEventListener("pageshow", resume);
    window.addEventListener("online", resume);
    function connect() {
      if (stopped) return;
      clearTimeout(resumeTimer);
      awaitingBaseline = true;
      setStatus("connecting");
      const url = new URL("ws", document.baseURI);
      if (projection.current) url.searchParams.set("colors", "1");
      url.protocol = location.protocol === "https:" ? "wss:" : "ws:";
      const ws = new WebSocket(url);
      socket.current = ws;
      ws.onopen = () => {
        if (stopped || socket.current !== ws) return;
        attempts = 0;
        setStatus("online");
      };
      let baseline = true;
      ws.onmessage = (event) => {
        if (stopped || socket.current !== ws) return;
        const data = parseFrame(String(event.data));
        if (!data) return;
        if (data.kind === "activity") {
          setBotActivities((items) =>
            [
              ...items.filter(
                (item) =>
                  item.expires > Date.now() / 1000 &&
                  !(
                    item.account_id === data.account_id &&
                    item.conversation_id === data.conversation_id
                  ),
              ),
              ...(data.state === "working" ? [data] : []),
            ].slice(-128),
          );
          return;
        }
        if (data.kind === "bot_commands") {
          setBotCommandVersions((previous) =>
            new Map(previous).set(
              data.conversation_id,
              (previous.get(data.conversation_id) ?? 0) + 1,
            ),
          );
          return;
        }
        if (data.kind === "snapshot") {
          awaitingBaseline = false;
          clearTimeout(resumeTimer);
          const ids = new Set([
            ...data.rooms.map((room) => room.id),
            ...Object.values(data.private_access).map((access) => access.id),
          ]);
          setBotCommandVersions(
            (previous) => new Map([...previous].filter(([id]) => ids.has(id))),
          );
          setBotActivities((items) =>
            items.filter((item) => ids.has(item.conversation_id)),
          );
          sync.receive(boundSnapshot(data), baseline);
          baseline = false;
        } else if (data.kind === "conversation") {
          const next = applyConversation(currentState.current, data);
          if (!next) {
            sync.refresh().catch(handleSyncError);
            return;
          }
          sync.receive(next, false);
        } else if (data.kind === "avatar") {
          sync.receive(applyAvatar(currentState.current, data), true);
        } else if (data.kind === "presence") {
          sync.receive({ ...currentState.current, online: data.online }, true);
        } else if (data.kind === "read") {
          sync.receive({ ...currentState.current, unread: data.unread }, true);
        } else if (data.kind === "notice" || data.kind === "error") {
          callbacks.current.onResult(data);
        }
      };
      ws.onclose = () => {
        if (stopped || socket.current !== ws) return;
        clearTimeout(resumeTimer);
        setStatus("offline");
        setBotActivities([]);
        callbacks.current.onDisconnect();
        if (stopped || socket.current !== ws) return;
        // Authentication and the backoff run together; a slow health check must
        // not add another network round trip before the socket can reconnect.
        retry = setTimeout(connect, Math.min(1000 * 2 ** attempts++, 15000));
        api.session().catch((error: unknown) => {
          if (!stopped && socket.current === ws) handleSyncError(error);
        });
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
      clearTimeout(resumeTimer);
      socket.current?.close();
    };
  }, []);
  useEffect(() => {
    if (projection.current === nameColors) return;
    const change = () => changeProjection.current(nameColors);
    if (callbacks.current.whenIdle) return callbacks.current.whenIdle(change);
    // A caller without a drain barrier must fail outstanding work explicitly;
    // the deliberately detached old socket will not deliver its close callback.
    callbacks.current.onDisconnect();
    change();
    return undefined;
  }, [nameColors]);

  return {
    state,
    currentState,
    status,
    socket,
    botCommandVersions,
    botActivities,
  };
}
