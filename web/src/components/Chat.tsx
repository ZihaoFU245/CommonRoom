import { controlMotion } from "../features/preferences/motion.ts";
import { useLocalConsole } from "../hooks/useLocalConsole.ts";
import { useCommands } from "../hooks/useCommands.ts";
import { useReadTracking } from "../hooks/useReadTracking.ts";
import { timelineAtBottom } from "../features/conversation/timelineScroll.ts";
import { useEffect, useRef, useState, useMemo } from "preact/hooks";
import type { Snapshot, Message } from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import {
  notifyMentions,
  toggleMentionNotifications,
} from "../features/preferences/notifications.ts";
import {
  timeline,
  clearedOrder,
  privatePeers,
  privateMessages,
} from "../features/conversation/console.ts";
import { DeleteConfirmation } from "./DeleteConfirmation.tsx";
import { Sidebar } from "./Sidebar.tsx";
import { OnlineUsers } from "./OnlineUsers.tsx";
import { Settings } from "./Settings.tsx";
import { BotProgress } from "./BotProgress.tsx";
import { Composer } from "./Composer.tsx";
import { MessageList } from "./MessageList.tsx";
import { useConnection } from "../hooks/useConnection.ts";
import { useBotCommands } from "../hooks/useBotCommands.ts";
import { useHistory } from "../hooks/useHistory.ts";
import { useComposerResize } from "../hooks/useComposerResize.ts";
import { useFonts } from "../hooks/useFonts.ts";
import {
  loadMarkdownHtml,
  saveMarkdownHtml,
} from "../features/preferences/security.ts";
import type { useTheme } from "../hooks/useTheme.ts";
import type { VirtualTimelineHandle } from "../hooks/useVirtualTimeline.ts";
import { useEvent } from "../hooks/useEvent.ts";
import autoAnimate from "@formkit/auto-animate";
import {
  loadLatex,
  loadMessageBars,
  loadMessageBarOpacity,
  loadNameColors,
  loadNameBorders,
  saveLatex,
  saveMessageBars,
  saveMessageBarOpacity,
  saveNameColors,
  saveNameBorders,
} from "../features/preferences/formatting.ts";
import {
  sameColors,
  resolvedColors,
  scopedColors,
  type NameColor,
} from "../features/preferences/nameColors.ts";
import { NameColorContext } from "./NameColorContext.ts";

export function Chat({
  initial,
  onLogout,
  appearance,
}: {
  initial: Snapshot;
  onLogout: () => void;
  appearance: ReturnType<typeof useTheme>;
}) {
  const [fonts, setFonts] = useFonts();
  const [latex, setLatex] = useState(loadLatex);
  const [showNameColors, setShowNameColors] = useState(loadNameColors);
  const [messageBars, setMessageBars] = useState(loadMessageBars);
  const [messageBarOpacity, setMessageBarOpacity] = useState(
    loadMessageBarOpacity,
  );
  const [nameBorders, setNameBorders] = useState(loadNameBorders);
  const [allowHtml, setAllowHtml] = useState(loadMarkdownHtml);
  const { theme, setTheme, seeds, setSeeds } = appearance;
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [selected, setSelected] = useState(() => {
    const requested =
      new URLSearchParams(location.hash.slice(1)).get("room") || "";
    return (requested?.startsWith("@direct:") &&
      privatePeers(initial).includes(requested.slice(8))) ||
      requested === "@command" ||
      initial.rooms.some((room) => room.name === requested)
      ? requested
      : initial.rooms[0]?.name || "@command";
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
  const notificationRequest = useRef(false);
  async function toggleNotifications() {
    if (notificationRequest.current) return;
    notificationRequest.current = true;
    try {
      const enabled = await toggleMentionNotifications(
        notificationsRef.current,
        window.Notification,
        window.isSecureContext,
      );
      notificationsRef.current = enabled;
      setNotifications(enabled);
      try {
        localStorage.setItem(
          "commonroom-notifications",
          enabled ? "on" : "off",
        );
      } catch {
        // The toggle still works for this session without browser storage.
      }
    } catch (error) {
      append("Notifications", errorMessage(error), true);
    } finally {
      notificationRequest.current = false;
    }
  }
  useEffect(() => {
    const checkPermission = () => {
      if (window.Notification?.permission !== "granted") {
        notificationsRef.current = false;
        setNotifications(false);
      }
    };
    window.addEventListener("focus", checkPermission);
    return () => window.removeEventListener("focus", checkPermission);
  }, []);
  const [menu, setMenu] = useState(() => {
    try {
      const saved = localStorage.getItem("commonroom-sidebar-open");
      if (saved === "true" || saved === "false") return saved === "true";
    } catch {
      // Use the responsive default when browser storage is unavailable.
    }
    return window.matchMedia("(min-width: 701px)").matches;
  });
  useEffect(() => {
    try {
      localStorage.setItem("commonroom-sidebar-open", String(menu));
    } catch {
      // Navigation remains usable when browser storage is unavailable.
    }
  }, [menu]);
  const menuButton = useRef<HTMLButtonElement>(null);
  const animatedLayout = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!animatedLayout.current) return;
    const animation = autoAnimate(animatedLayout.current, controlMotion);
    return () => animation.destroy?.();
  }, []);
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
  const virtualTimeline = useRef<VirtualTimelineHandle>(null);
  const atBottom = useRef(true);
  const input = useRef<HTMLTextAreaElement>(null);
  const { output, cleared, setCleared, ledger, append, complete, clearView } =
    useLocalConsole(initial, selected, selectedRef, atBottom);
  const {
    state,
    currentState,
    status,
    socket,
    botCommandVersions,
    botActivities,
  } = useConnection(
    initial,
    {
      onLogout,
      onSnapshot(previous, data, baseline) {
        const view = selectedRef.current;
        const oldId =
          previous.private_access[view]?.id ??
          previous.rooms.find((room) => room.name === view)?.id;
        if (
          oldId &&
          !data.private_access[view] &&
          !data.rooms.some((room) => room.name === view)
        ) {
          const renamedView =
            Object.keys(data.private_access).find(
              (key) => data.private_access[key]?.id === oldId,
            ) ?? data.rooms.find((room) => room.id === oldId)?.name;
          if (renamedView) setSelected(renamedView);
        }
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
      whenIdle: (callback) => commands.whenIdle(callback),
    },
    showNameColors,
  );
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
        ? privateMessages(
            state.direct,
            state.username,
            peer,
            state.private_access[selected]?.id,
          )
        : room?.messages || [],
    [
      direct,
      peer,
      selected,
      state.direct,
      state.username,
      state.private_access,
      room?.messages,
    ],
  );
  const commands = useCommands({
    botCommands: useBotCommands(
      room?.id ?? state.private_access[selected]?.id,
      botCommandVersions,
      `${state.account_access.id}:${state.sync?.instance ?? ""}:${state.policy_revision}`,
      useMemo(
        () =>
          new Map([
            ...state.rooms.map((item): [string, string] => [
              item.id,
              item.members.join("\0"),
            ]),
            ...Object.entries(state.private_access).map(
              ([view, access]): [string, string] => [access.id, view],
            ),
          ]),
        [state.rooms, state.private_access],
      ),
      status === "online",
    ),
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
    selectCaret,
    acceptHint,
    reactTo,
  } = commands;
  const {
    minimumComposerHeight,
    actualComposerHeight,
    startResize,
    moveResize,
    endResize,
    resizeComposer,
  } = useComposerResize(fonts.chat, input, draft);
  const {
    unread,
    historyReady,
    nameColors,
    messages,
    historyError,
    lookahead,
    retryHistory,
    loading,
    refreshing,
    hasBefore,
    hasAfter,
    loadOlder,
    loadNewer,
    loadLatest,
    loadSequence,
  } = useHistory(
    state,
    selected,
    tail,
    currentState,
    ledger,
    virtualTimeline,
    atBottom,
    showNameColors,
  );
  const viewColors = showNameColors
    ? resolvedColors(
        scopedColors(state.colors, room?.colors),
        state.color_version,
        nameColors,
        room?.name_colors ?? (direct ? state.name_colors : undefined),
      )
    : {};
  const colors = useRef(viewColors);
  if (!sameColors(colors.current, viewColors)) colors.current = viewColors;
  const { unreadBoundary, acknowledgeVisible, rememberPosition, jumpToUnread } =
    useReadTracking({
      selected,
      conversationId: room?.id ?? state.private_access[selected]?.id,
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
      loadSequence,
      virtualTimeline,
      atBottom,
      historyElement,
    });
  const entries = useMemo(
    () =>
      timeline(
        messages,
        output,
        ledger.current,
        clearedOrder(cleared, selected),
        selected,
      ),
    [messages, output, cleared, selected],
  );
  const handleReact = useEvent(reactTo);
  const handleEdit = useEvent(commands.editMessage);
  const handleDelete = useEvent((message: Message) =>
    commands.retract(message),
  );
  const handleReply = useEvent((message: Message) => {
    setReplyTarget({ ...message, view: selected });
    input.current?.focus();
  });
  const handleLoadSequence = useEvent(loadSequence);
  const handleScroll = useEvent(() => {
    const element = historyElement.current;
    if (element)
      atBottom.current = timelineAtBottom(element, atBottom.current === true);
    rememberPosition();
    acknowledgeVisible();
    const prediction = element
      ? lookahead.observe(
          element.scrollTop,
          element.clientHeight,
          performance.now(),
        )
      : null;
    if (element && prediction && !loading && historyReady) {
      if (
        prediction.direction < 0 &&
        element.scrollTop < prediction.distance &&
        hasBefore
      )
        loadOlder();
      else if (
        prediction.direction > 0 &&
        element.scrollHeight - element.scrollTop - element.clientHeight <
          prediction.distance &&
        hasAfter
      )
        loadNewer();
    }
  });
  useEffect(() => {
    if (
      (peer && !peers.includes(peer)) ||
      (!direct && !consoleView && !state.rooms.some((r) => r.name === selected))
    )
      setSelected(
        state.rooms[0]?.name || (peers[0] ? `@direct:${peers[0]}` : "@command"),
      );
  }, [state.rooms, selected, peers]);
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
    rememberPosition();
    setSelected(name);
    if (window.matchMedia("(max-width: 700px)").matches) setMenu(false);
    input.current?.focus();
  }
  async function changeNameColor(color: NameColor | null, room: string | null) {
    await api.color(color, room);
  }
  const view = (
    <div
      class={`chat-shell ${menu ? "navigation-open" : ""} ${messageBars ? "show-message-bars" : ""} ${nameBorders ? "show-name-borders" : ""}`}
      style={{ "--message-bar-opacity": `${messageBarOpacity}%` }}
    >
      <button
        class="mobile-menu navigation-toggle"
        ref={menuButton}
        aria-label={menu ? "Close navigation" : "Open navigation"}
        aria-expanded={menu}
        aria-controls="navigation"
        onClick={() => (menu ? closeMenu() : setMenu(true))}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d={menu ? "m6 6 12 12M18 6 6 18" : "M5 6h14M5 12h14M5 18h14"} />
        </svg>
      </button>
      {commands.deleteConfirmation && (
        <DeleteConfirmation
          kind={commands.deleteConfirmation}
          onCancel={commands.cancelDelete}
          onConfirm={commands.confirmDelete}
        />
      )}
      {settingsOpen && (
        <Settings
          state={state}
          pending={commands.pending}
          onAccountCommand={commands.accountCommand}
          aliases={commands.aliases}
          onSaveAlias={commands.saveAliasMapping}
          onRemoveAlias={commands.removeAliasMapping}
          fonts={fonts}
          theme={theme}
          seeds={seeds}
          markdown={commands.markdown}
          onMarkdownChange={commands.changeMarkdown}
          latex={latex}
          onLatexChange={(enabled) => {
            setLatex(enabled);
            saveLatex(enabled);
          }}
          nameColors={showNameColors}
          onNameColorsChange={(enabled) => {
            setShowNameColors(enabled);
            saveNameColors(enabled);
          }}
          messageBarOpacity={messageBarOpacity}
          onMessageBarOpacityChange={(opacity) => {
            setMessageBarOpacity(opacity);
            saveMessageBarOpacity(opacity);
          }}
          messageBars={messageBars}
          onMessageBarsChange={(enabled) => {
            setMessageBars(enabled);
            saveMessageBars(enabled);
          }}
          nameBorders={nameBorders}
          onNameBordersChange={(enabled) => {
            setNameBorders(enabled);
            saveNameBorders(enabled);
          }}
          allowHtml={allowHtml}
          onAllowHtmlChange={(enabled) => {
            setAllowHtml(enabled);
            saveMarkdownHtml(enabled);
          }}
          onChange={setFonts}
          onThemeChange={setTheme}
          onSeedsChange={setSeeds}
          onNameColorChange={changeNameColor}
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
      <div class="chat-layout" ref={animatedLayout}>
        {menu && (
          <Sidebar
            state={state}
            selected={selected}
            peers={peers}
            peer={peer}
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
        )}
        <main class="conversation">
          <header class="chat-header">
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
              <OnlineUsers
                names={state.online}
                username={state.username}
                connected={status === "online"}
              />
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
          {(hasBefore || hasAfter || (loading && !refreshing)) && (
            <div class="history-navigation">
              {hasBefore && (
                <button disabled={loading} onClick={loadOlder}>
                  Older messages
                </button>
              )}
              {loading && !refreshing && (
                <span role="status">Loading history…</span>
              )}
              {hasAfter && (
                <>
                  <button disabled={loading} onClick={loadNewer}>
                    Newer messages
                  </button>
                  <button
                    disabled={loading}
                    onClick={() => {
                      atBottom.current = true;
                      loadLatest();
                    }}
                  >
                    Latest messages
                  </button>
                </>
              )}
            </div>
          )}
          {historyError && (
            <div class="history-error" role="alert">
              {historyError}
              <button onClick={() => retryHistory()}>Retry history</button>
            </div>
          )}
          <MessageList
            messageBars={messageBars}
            avatars={state.avatars}
            motionScope={
              state.private_access[selected]?.id ??
              state.rooms.find((room) => room.name === selected)?.id ??
              selected
            }
            revision={unread?.revision ?? 0}
            historyReady={historyReady}
            atBottom={atBottom}
            debug={commands.debug}
            allowHtml={allowHtml}
            latex={latex}
            permissions={commands.permissions}
            commands={commands.serverCommands}
            entries={entries}
            username={state.username}
            pending={pending}
            unreadBoundary={unreadBoundary}
            onReact={handleReact}
            onEdit={handleEdit}
            onReply={handleReply}
            onDelete={handleDelete}
            loadSequence={handleLoadSequence}
            virtualTimeline={virtualTimeline}
            historyElement={historyElement}
            end={end}
            onScroll={handleScroll}
          />
          <BotProgress
            activities={botActivities.filter(
              (item) =>
                item.conversation_id ===
                (room?.id ?? state.private_access[selected]?.id),
            )}
          />
          {commands.outgoing.some((item) => item.view === selected) && (
            <div class="outgoing-messages" aria-live="polite">
              {commands.outgoing
                .filter((item) => item.view === selected)
                .map((item) => (
                  <div key={item.key} class="outgoing-message">
                    <p dir="auto">{item.text}</p>
                    <small>{item.error ?? "Sending…"}</small>
                    {item.error && (
                      <button
                        type="button"
                        disabled={!!commands.draft}
                        onClick={() => commands.restoreOutgoing(item.key)}
                      >
                        Edit and resend
                      </button>
                    )}
                  </div>
                ))}
            </div>
          )}
          <Composer
            mediaUpload={commands.mediaUpload}
            canWrite={commands.permissions.includes("w:message.create")}
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
            selectCaret={selectCaret}
          />
        </main>
      </div>
    </div>
  );
  return (
    <NameColorContext.Provider value={colors.current}>
      {view}
    </NameColorContext.Provider>
  );
}
