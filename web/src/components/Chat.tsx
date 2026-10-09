import { useLocalConsole } from "../hooks/useLocalConsole.ts";
import { useCommands } from "../hooks/useCommands.ts";
import { useReadTracking } from "../hooks/useReadTracking.ts";
import { useEffect, useRef, useState, useMemo } from "preact/hooks";
import type { Snapshot, Message, History } from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { notifyMentions } from "../features/preferences/notifications.ts";
import { retainedHistory } from "../features/conversation/unread.ts";
import {
  ingest,
  timeline,
  privatePeers,
  privateMessages,
} from "../features/conversation/console.ts";
import { DeleteConfirmation } from "./DeleteConfirmation.tsx";
import { Sidebar } from "./Sidebar.tsx";
import { Settings } from "./Settings.tsx";
import { Composer } from "./Composer.tsx";
import { MessageList } from "./MessageList.tsx";
import { useConnection } from "../hooks/useConnection.ts";
import { useHistory } from "../hooks/useHistory.ts";
import { useComposerResize } from "../hooks/useComposerResize.ts";
import { useFonts } from "../hooks/useFonts.ts";
import { useEvent } from "../hooks/useEvent.ts";

export function Chat({
  initial,
  onLogout,
}: {
  initial: Snapshot;
  onLogout: () => void;
}) {
  const [fonts, setFonts] = useFonts();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [selected, setSelected] = useState(() => {
    const requested =
      new URLSearchParams(location.hash.slice(1)).get("room") || "";
    return (requested?.startsWith("@direct:") &&
      privatePeers(initial).includes(requested.slice(8))) ||
      (requested === "@command" && initial.admin) ||
      initial.rooms.some((room) => room.name === requested)
      ? requested
      : initial.admin
        ? "@command"
        : initial.rooms[0]?.name || "@direct";
  });
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
  const [notifications, setNotifications] = useState(() => {
    try {
      return (
        typeof Notification !== "undefined" &&
        Notification.permission === "granted" &&
        localStorage.getItem("commonroom-notifications") === "on"
      );
    } catch {
      return false;
    }
  });
  const notificationsRef = useRef(notifications);
  async function toggleNotifications() {
    if (!("Notification" in window) || !window.isSecureContext) {
      append(
        "Notifications",
        "Desktop notifications require a supported browser and HTTPS (or localhost).",
        true,
      );
      return;
    }
    try {
      const enabled =
        !notifications &&
        (Notification.permission === "granted" ||
          (await Notification.requestPermission()) === "granted");
      notificationsRef.current = enabled;
      setNotifications(enabled);
      localStorage.setItem("commonroom-notifications", enabled ? "on" : "off");
      if (!enabled && Notification.permission === "denied")
        append(
          "Notifications",
          "Notifications are blocked. Change permission in Chrome site settings.",
          true,
        );
    } catch {
      append(
        "Notifications",
        "This browser cannot enable notifications.",
        true,
      );
    }
  }
  const [menu, setMenu] = useState(
    () => window.matchMedia("(min-width: 701px)").matches,
  );
  const menuButton = useRef<HTMLButtonElement>(null);
  function closeMenu() {
    setMenu(false);
    requestAnimationFrame(() => menuButton.current?.focus());
  }
  useEffect(() => {
    if (!menu || settingsOpen) return;
    const close = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeMenu();
    };
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [menu, settingsOpen]);
  const desiredRoom = useRef<string | null>(null);
  const end = useRef<HTMLDivElement>(null);
  const historyElement = useRef<HTMLElement>(null);
  const historyCache = useRef<History | null>(null);
  const atBottom = useRef(true);
  const input = useRef<HTMLTextAreaElement>(null);
  const { output, cleared, setCleared, ledger, append, complete, clearView } =
    useLocalConsole(initial, selected, selectedRef, atBottom);
  const {
    minimumComposerHeight,
    actualComposerHeight,
    startResize,
    moveResize,
    endResize,
    resizeComposer,
  } = useComposerResize(fonts.chat, input);
  const { state, currentState, status, socket } = useConnection(initial, {
    onLogout,
    onSnapshot(previous, data, baseline) {
      const contentChanged =
        previous.rooms !== data.rooms || previous.direct !== data.direct;
      if (contentChanged)
        notifyMentions(previous, data, {
          baseline,
          enabled: notificationsRef.current,
          NotificationClass: window.Notification,
          background: document.hidden || !document.hasFocus(),
          selected: selectedRef.current,
          open: (view) => {
            window.focus();
            setSelected(view);
          },
        });
      const cache = historyCache.current;
      const metadata = cache && data.unread?.[cache.view];
      if (contentChanged)
        ingest(
          data,
          ledger.current,
          metadata ? retainedHistory(cache.messages, [], metadata) : [],
        );
      if (
        desiredRoom.current &&
        (data.rooms.some((r) => r.name === desiredRoom.current) ||
          (desiredRoom.current.startsWith("@direct:") &&
            privatePeers(data).includes(desiredRoom.current.slice(8))))
      ) {
        setSelected(desiredRoom.current);
        desiredRoom.current = null;
      }
    },
    onResult: (data) => commands.receive(data),
    onDisconnect: () => commands.disconnect(),
  });
  const room = state.rooms.find((r) => r.name === selected);
  const peers = useMemo(
    () => privatePeers(state),
    [state.private_peers, state.direct, state.username],
  );
  const direct = selected === "@direct" || selected.startsWith("@direct:");
  const peer = selected.startsWith("@direct:") ? selected.slice(8) : null;
  const consoleView = selected === "@command";
  const tail = useMemo(
    () =>
      direct
        ? privateMessages(state.direct, state.username, peer)
        : room?.messages || [],
    [direct, peer, state.direct, state.username, room?.messages],
  );
  const commands = useCommands({
    selected,
    direct,
    consoleView,
    peer,
    state,
    room,
    socket,
    currentState,
    input,
    desiredRoom,
    setSelected,
    append,
    complete,
    clearView,
    logout: () => {
      logout().catch(reportUnexpectedError);
    },
  });
  const {
    draft,
    pending,
    replyTarget,
    setReplyTarget,
    hints,
    activeHint,
    send,
    edit,
    keydown,
    acceptHint,
    reactTo,
  } = commands;
  const { unread, historyReady, messages, historyError, retryHistory } =
    useHistory(
      state,
      selected,
      tail,
      historyCache,
      currentState,
      selectedRef,
      ledger,
    );
  const { unreadBoundary, acknowledgeVisible, jumpToUnread, resetEntry } =
    useReadTracking({
      selected,
      unread,
      messages,
      historyReady,
      output,
      cleared,
      resetCleared: () =>
        setCleared((previous) => ({ ...previous, [selected]: 0 })),
      settingsOpen,
      menu,
      status,
      retryHistory,
      atBottom,
      historyElement,
      end,
    });
  const entries = useMemo(
    () =>
      timeline(
        messages,
        output,
        ledger.current,
        cleared[selected] || 0,
        selected,
      ),
    [messages, output, cleared, selected],
  );
  const handleReact = useEvent(reactTo);
  const handleDelete = useEvent((message: Message) =>
    commands.retract(message),
  );
  const handleReply = useEvent((message: Message) => {
    setReplyTarget({ ...message, view: selected });
    input.current?.focus();
  });
  const handleScroll = useEvent(() => {
    const element = historyElement.current;
    if (element)
      atBottom.current =
        element.scrollHeight - element.scrollTop - element.clientHeight < 32;
    acknowledgeVisible();
  });
  useEffect(() => {
    if (
      (peer && !peers.includes(peer)) ||
      (!direct &&
        !(consoleView && state.admin) &&
        !state.rooms.some((r) => r.name === selected))
    )
      setSelected(
        state.admin
          ? "@command"
          : state.rooms[0]?.name ||
              (peers[0] ? `@direct:${peers[0]}` : "@direct"),
      );
  }, [state.rooms, state.admin, selected, peers]);
  useEffect(() => {
    history.replaceState(null, "", `#room=${encodeURIComponent(selected)}`);
  }, [selected]);
  async function logout() {
    try {
      await api.logout();
      onLogout();
    } catch (error) {
      append("/logout", errorMessage(error), true);
    }
  }
  function reportUnexpectedError(error: unknown) {
    append("Error", errorMessage(error), true);
  }
  function choose(name: string) {
    resetEntry();
    setSelected(name);
    if (window.matchMedia("(max-width: 700px)").matches) setMenu(false);
    input.current?.focus();
  }
  return (
    <div class="chat-layout">
      {commands.deleteConfirmation && (
        <DeleteConfirmation
          onCancel={commands.cancelDelete}
          onConfirm={commands.confirmDelete}
        />
      )}
      <Sidebar
        state={state}
        menu={menu}
        consoleView={consoleView}
        selected={selected}
        peers={peers}
        peer={peer}
        closeMenu={closeMenu}
        choose={choose}
        openSettings={() => setSettingsOpen(true)}
        toggleNotifications={() => {
          toggleNotifications().catch(reportUnexpectedError);
        }}
        notifications={notifications}
        logout={() => {
          logout().catch(reportUnexpectedError);
        }}
      />
      {settingsOpen && (
        <Settings
          fonts={fonts}
          onChange={setFonts}
          onClose={() => setSettingsOpen(false)}
        />
      )}
      {menu && (
        <button
          class="scrim"
          aria-label="Close navigation"
          onClick={closeMenu}
        />
      )}
      <main class="conversation">
        <header class="chat-header">
          <button
            class="mobile-menu"
            hidden={menu}
            ref={menuButton}
            aria-label={menu ? "Close navigation" : "Open navigation"}
            aria-expanded={menu}
            aria-controls="navigation"
            onClick={() => (menu ? closeMenu() : setMenu(true))}
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M5 6h14M5 12h14M5 18h14" />
            </svg>
          </button>
          <div class="room-title">
            <span>{consoleView ? ">" : direct ? "↗" : "#"}</span>
            <h1>
              {consoleView
                ? "Command"
                : direct
                  ? peer || "Private messages"
                  : selected}
            </h1>
          </div>
          <div class="header-actions">
            <details class="online-users">
              <summary>
                {status === "online"
                  ? `${state.online.length} online`
                  : "Online users"}
              </summary>
              <div class="online-list">
                {status === "online" ? (
                  state.online.map((name) => (
                    <span key={name}>
                      {name}
                      {name === state.username ? " (you)" : ""}
                    </span>
                  ))
                ) : (
                  <span>Reconnecting…</span>
                )}
              </div>
            </details>
            <span
              class={`connection ${status}`}
              role="status"
              aria-label={
                status === "online"
                  ? "Connected"
                  : status === "connecting"
                    ? "Connecting"
                    : "Reconnecting"
              }
            >
              <i />
              {status === "online"
                ? "Connected"
                : status === "connecting"
                  ? "Connecting"
                  : "Reconnecting"}
            </span>
          </div>
        </header>
        {unread && unread.count > 0 && (
          <div class="unread-bar">
            <span>{unread.count} unread</span>
            <button onClick={jumpToUnread}>Jump to unread ↓</button>
          </div>
        )}
        {historyError && (
          <div class="history-error" role="alert">
            {historyError}
            <button onClick={() => retryHistory()}>Retry history</button>
          </div>
        )}
        <MessageList
          entries={entries}
          username={state.username}
          pending={pending}
          unreadBoundary={unreadBoundary}
          onReact={handleReact}
          onReply={handleReply}
          onDelete={handleDelete}
          historyElement={historyElement}
          end={end}
          onScroll={handleScroll}
        />
        <Composer
          minimumComposerHeight={minimumComposerHeight}
          actualComposerHeight={actualComposerHeight}
          hints={hints}
          activeHint={activeHint}
          replyTarget={replyTarget}
          input={input}
          consoleView={consoleView}
          draft={draft}
          pending={pending}
          startResize={startResize}
          moveResize={moveResize}
          endResize={endResize}
          resizeComposer={resizeComposer}
          acceptHint={acceptHint}
          cancelReply={() => setReplyTarget(null)}
          send={send}
          edit={edit}
          keydown={keydown}
        />
      </main>
    </div>
  );
}
