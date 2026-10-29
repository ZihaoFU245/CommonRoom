import { controlMotion } from "../features/preferences/motion.ts";
import { useLocalConsole } from "../hooks/useLocalConsole.ts";
import { useCommands } from "../hooks/useCommands.ts";
import { useReadTracking } from "../hooks/useReadTracking.ts";
import { useHistoryNoticeMotion } from "../hooks/useHistoryNoticeMotion.ts";
import { useHistoryPaging } from "../hooks/useHistoryPaging.ts";
import { timelineAtBottom } from "../features/conversation/timelineScroll.ts";
import { useEffect, useRef, useState, useMemo } from "preact/hooks";
import type { Snapshot, Message } from "../api/protocol.ts";
import { errorMessage } from "../api/protocol.ts";
import { api } from "../api/client.ts";
import { notifyMentions } from "../features/preferences/notifications.ts";
import {
  timeline,
  clearedOrder,
  privatePeers,
  privateMessages,
} from "../features/conversation/console.ts";
import { DeleteConfirmation } from "./DeleteConfirmation.tsx";
import { Sidebar } from "./Sidebar.tsx";
import { ChatHeader } from "./ChatHeader.tsx";
import { OutgoingPane } from "./OutgoingPane.tsx";
import { Settings } from "./Settings.tsx";
import { BotProgress } from "./BotProgress.tsx";
import { Composer } from "./Composer.tsx";
import { MessageList } from "./MessageList.tsx";
import { useConnection } from "../hooks/useConnection.ts";
import { useBotCommands } from "../hooks/useBotCommands.ts";
import { useHistory } from "../hooks/useHistory.ts";
import { useComposerResize } from "../hooks/useComposerResize.ts";
import { useChatPreferences } from "../hooks/useChatPreferences.ts";
import { useChatNotifications } from "../hooks/useChatNotifications.ts";
import type { useTheme } from "../hooks/useTheme.ts";
import type { VirtualTimelineHandle } from "../hooks/useVirtualTimeline.ts";
import { useEvent } from "../hooks/useEvent.ts";
import autoAnimate from "@formkit/auto-animate";
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
  const {
    fonts,
    setFonts,
    latex,
    setLatex,
    showNameColors,
    setShowNameColors,
    messageBars,
    setMessageBars,
    messageBarOpacity,
    setMessageBarOpacity,
    nameBorders,
    setNameBorders,
    allowHtml,
    setAllowHtml,
  } = useChatPreferences();
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
  const initialSelection = useRef(selected);
  const selectedRef = useRef(selected);
  selectedRef.current = selected;
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
  const historyNotices = useHistoryNoticeMotion();
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
  const { notifications, notificationsRef, toggleNotifications } =
    useChatNotifications(append);

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
        const contentChanged = Object.entries(data.unread).some(
          ([view, unread]) =>
            unread.through > (previous.unread[view]?.through ?? 0),
        );
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
    () =>
      privatePeers({
        username: state.username,
        direct: state.direct,
        private_peers: state.private_peers,
      }),
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
    renderLedger,
    nameColors,
    messages,
    historyError,
    lookahead,
    retryHistory,
    loading,
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
  useHistoryPaging(
    historyElement,
    historyReady,
    loading,
    hasBefore,
    hasAfter,
    loadOlder,
    loadNewer,
  );
  const colors = useRef(viewColors);
  if (!sameColors(colors.current, viewColors)) colors.current = viewColors;
  const {
    unreadBoundary,
    acknowledgeVisible,
    rememberPosition,
    jumpToUnread,
    jumpToBottom,
    showJumpToBottom,
    showJumpToUnread,
    updateNavigation,
  } = useReadTracking({
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
    hasAfter,
    loadLatest,
    historyLoading: loading,
  });
  const entries = useMemo(
    () =>
      timeline(
        messages,
        output,
        renderLedger,
        clearedOrder(cleared, selected),
        selected,
      ),
    [messages, output, cleared, selected, renderLedger],
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
    const scrolling = virtualTimeline.current?.isScrolling?.() ?? false;
    if (element)
      atBottom.current =
        !scrolling && timelineAtBottom(element, atBottom.current === true);
    rememberPosition();
    acknowledgeVisible();
    updateNavigation();
    if (scrolling) {
      lookahead.reset();
      return;
    }
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
  }, [state.rooms, selected, peers, peer, direct, consoleView]);
  useEffect(() => {
    const params = new URLSearchParams(location.hash.slice(1));
    if (
      params.get("room") === selected ||
      (!params.has("room") && selected === initialSelection.current)
    )
      return;
    params.set("room", selected);
    history.replaceState(history.state, "", `#${params.toString()}`);
  }, [selected]);
  const restoreSelection = useEvent(() => {
    const requested =
      new URLSearchParams(location.hash.slice(1)).get("room") ||
      initialSelection.current;
    if (
      requested &&
      (requested === "@command" ||
        state.rooms.some((room) => room.name === requested) ||
        (requested.startsWith("@direct:") &&
          peers.includes(requested.slice(8))))
    ) {
      rememberPosition();
      setSelected(requested);
    }
  });
  useEffect(() => {
    window.addEventListener("popstate", restoreSelection);
    window.addEventListener("hashchange", restoreSelection);
    return () => {
      window.removeEventListener("popstate", restoreSelection);
      window.removeEventListener("hashchange", restoreSelection);
    };
  }, [restoreSelection]);
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
    if (name !== selected) {
      const params = new URLSearchParams(location.hash.slice(1));
      params.set("room", name);
      history.pushState(history.state, "", `#${params.toString()}`);
    }
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
          onLatexChange={setLatex}
          nameColors={showNameColors}
          onNameColorsChange={setShowNameColors}
          messageBarOpacity={messageBarOpacity}
          onMessageBarOpacityChange={setMessageBarOpacity}
          messageBars={messageBars}
          onMessageBarsChange={setMessageBars}
          nameBorders={nameBorders}
          onNameBordersChange={setNameBorders}
          allowHtml={allowHtml}
          onAllowHtmlChange={setAllowHtml}
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
          <ChatHeader
            consoleView={consoleView}
            direct={direct}
            peer={peer}
            selected={selected}
            online={state.online}
            username={state.username}
            status={status}
          />
          <div class="history-notices" ref={historyNotices}>
            {(showJumpToUnread || showJumpToBottom) && (
              <div class="unread-bar" key={selected}>
                <button
                  onClick={showJumpToUnread ? jumpToUnread : jumpToBottom}
                >
                  {showJumpToUnread ? "Jump to unread ↓" : "Jump to bottom ↓"}
                </button>
              </div>
            )}
          </div>
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
          <OutgoingPane
            items={commands.outgoing}
            selected={selected}
            draft={commands.draft}
            restore={commands.restoreOutgoing}
          />
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
