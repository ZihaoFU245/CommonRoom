import { UserCardTrigger } from "./UserCardTrigger.tsx";
import { DisplayName } from "./DisplayName.tsx";
import { Avatar } from "./Avatar.tsx";
import type { Snapshot } from "../api/protocol.ts";
import type { RefObject } from "preact";
import { Brand } from "./Brand.tsx";
import { UnreadBadge } from "./UnreadBadge.tsx";
import { ChatActionIcon, ChatSwipeRow } from "./ChatSwipeRow.tsx";
import { MessageActionMenu } from "./MessageActionMenu.tsx";
import {
  conversationActivity,
  conversationList,
  conversationPreview,
} from "../features/conversation/conversationList.ts";
import { messageDate } from "../features/conversation/messages.ts";
import { useSettingsMotion as useConversationListMotion } from "../hooks/useSettingsMotion.ts";
import type { useChatListPreferences } from "../hooks/useChatListPreferences.ts";
import { useChatRowMenu } from "../hooks/useChatRowMenu.ts";
import { useShortChatScroll } from "../hooks/useShortChatScroll.ts";
import { useId, useMemo, useState } from "preact/hooks";

const relativeDate = new Intl.RelativeTimeFormat(undefined, {
  numeric: "auto",
});
const initialSegmenter =
  typeof Intl.Segmenter === "function"
    ? new Intl.Segmenter(undefined, { granularity: "grapheme" })
    : undefined;

function chatInitial(name: string) {
  const text = name.trimStart();
  return (
    initialSegmenter?.segment(text).containing(0)?.segment ??
    text[Symbol.iterator]().next().value ??
    ""
  );
}

interface Props {
  mobile: boolean;
  active: boolean;
  heading: RefObject<HTMLHeadingElement>;
  state: Snapshot;
  selected: string;
  peers: string[];
  preferences: ReturnType<typeof useChatListPreferences>;
  markRead: (view: string) => Promise<void>;
  choose: (name: string) => void;
  openSettings: () => void;
  toggleNotifications: () => void;
  notifications: boolean;
  logout: () => void;
}
export function Sidebar({
  mobile,
  active,
  heading,
  state,
  selected,
  peers,
  preferences,
  markRead,
  choose,
  openSettings,
  toggleNotifications,
  notifications,
  logout,
}: Props) {
  const conversations = useMemo(() => {
    const pinPosition = (view: string) =>
      preferences.pinPosition(view) ?? Number.MAX_SAFE_INTEGER;
    return conversationList(
      state.rooms,
      peers,
      state.unread,
      state.display_names,
      preferences.isMarkedUnread,
    ).sort((a, b) => pinPosition(a.view) - pinPosition(b.view));
  }, [state.rooms, peers, state.unread, state.display_names, preferences]);
  const activity = useMemo(
    () => (mobile ? conversationActivity(state) : undefined),
    [mobile, state],
  );
  const showPinnedHeading =
    conversations[0] !== undefined &&
    preferences.pinPosition(conversations[0].view) !== undefined;
  const chatOrder = mobile
    ? JSON.stringify([
        showPinnedHeading,
        ...conversations.map(({ view }) => view),
      ])
    : "";
  const today = new Date();
  const todayKey = messageDate(today.getTime() / 1000).key;
  today.setDate(today.getDate() - 1);
  const yesterdayKey = messageDate(today.getTime() / 1000).key;
  const list = useConversationListMotion<HTMLOListElement>();
  const chatMenu = useChatRowMenu(mobile, active);
  const menuId = useId();
  const menuView =
    active &&
    chatMenu.menu &&
    conversations.some(({ view }) => view === chatMenu.menu?.view)
      ? chatMenu.menu.view
      : undefined;
  const pinned =
    menuView !== undefined && preferences.pinPosition(menuView) !== undefined;
  const marked = menuView !== undefined && preferences.isMarkedUnread(menuView);
  const menuUnread = Boolean(menuView && state.unread[menuView]?.count);
  const [readStatus, setReadStatus] = useState<{
    text: string;
    error?: true;
  }>();
  function runReadAction(view: string, read: boolean) {
    if (read) {
      setReadStatus(undefined);
      void markRead(view).then(
        () => setReadStatus({ text: "Chat marked as read." }),
        () =>
          setReadStatus({
            text: "Could not mark the chat as read. Try again.",
            error: true,
          }),
      );
    } else if (
      !state.unread[view]?.count &&
      !preferences.isMarkedUnread(view)
    ) {
      preferences.change("unread", view, true);
      setReadStatus({ text: "Chat marked as unread." });
    }
  }
  const shortScroll = useShortChatScroll(
    mobile && active && conversations.length > 0 && !menuView,
    list,
  );
  return (
    <aside id="navigation" class="sidebar visible">
      <div class="sidebar-heading">
        {mobile ? (
          <h1 ref={heading} tabIndex={-1}>
            Chats
          </h1>
        ) : (
          <Brand />
        )}
      </div>
      <nav
        class="navigation"
        aria-label={mobile ? "Chats" : "Conversations"}
        {...shortScroll}
      >
        <ol class="conversation-list" ref={list}>
          {showPinnedHeading && (
            <li key="pinned-heading" role="presentation">
              <h2 class="pinned-chats-heading">
                <ChatActionIcon kind="pin" /> Pinned chats
              </h2>
            </li>
          )}
          {conversations.map(({ view, kind, name }) => {
            const displayName =
              kind === "direct" &&
              state.display_names &&
              Object.hasOwn(state.display_names, name)
                ? (state.display_names[name] ?? name)
                : name;
            const unread = state.unread[view];
            const count = unread?.count ?? 0;
            const showDot = count === 0 && preferences.isMarkedUnread(view);
            const rowEvents = chatMenu.rowEvents(view);
            const badge = (showDot || count > 0) && (
              <span class="chat-unread-indicator">
                {showDot ? (
                  <span
                    class="chat-unread-dot"
                    role="img"
                    aria-label="Marked as unread"
                  />
                ) : (
                  <UnreadBadge count={count} />
                )}
              </span>
            );
            const recent = activity?.get(view);
            const latest = recent?.latest;
            const date =
              latest && (!unread || latest.sequence === unread.through)
                ? messageDate(latest.time)
                : undefined;
            const message = recent?.unread ?? (date ? latest : undefined);
            const preview = message && conversationPreview(message);
            const label = (
              <span class="chat-row-name" dir="auto">
                {kind === "direct" ? <DisplayName name={name} /> : name}
              </span>
            );
            const canRead = count > 0 || showDot;
            const chatButton = (
              <button
                key="chat"
                type="button"
                aria-current={selected === view ? "true" : undefined}
                class={`room-link ${selected === view ? "selected" : ""}`}
                onClick={() => choose(view)}
                {...rowEvents}
              >
                <span
                  class={`chat-row-icon ${kind === "direct" ? "private-chat-icon" : ""}`}
                  aria-hidden="true"
                >
                  <span class="hash">
                    {mobile
                      ? chatInitial(displayName)
                      : kind === "direct" || view.startsWith("@private:")
                        ? "↗"
                        : "#"}
                  </span>
                  {kind === "direct" && (
                    <Avatar url={state.avatars?.[name]} name={name} />
                  )}
                </span>
                {mobile ? (
                  <span class="chat-row-details">
                    <span class="chat-row-title">
                      {label}
                      {date?.iso && (
                        <time
                          class="chat-row-time"
                          dateTime={date.iso}
                          title={date.full}
                        >
                          {date.key === todayKey
                            ? date.time
                            : date.key === yesterdayKey
                              ? relativeDate.format(-1, "day")
                              : date.label}
                        </time>
                      )}
                    </span>
                    {(preview || count > 0 || showDot) && (
                      <span class="chat-row-summary">
                        {preview && message && (
                          <span class="chat-row-preview" dir="auto">
                            {message.from === state.username ? (
                              "You: "
                            ) : kind === "room" ? (
                              <>
                                <DisplayName name={message.from} />
                                {": "}
                              </>
                            ) : null}
                            {preview}
                          </span>
                        )}
                        {badge}
                      </span>
                    )}
                  </span>
                ) : (
                  <>
                    {label}
                    {badge}
                  </>
                )}
              </button>
            );
            return (
              <li
                class="chat-list-row"
                key={view}
                data-menu-open={menuView === view || undefined}
                data-pinned={
                  preferences.pinPosition(view) !== undefined || undefined
                }
              >
                {mobile ? (
                  <ChatSwipeRow
                    canRead={canRead}
                    enabled={active && !menuView}
                    chatOrder={chatOrder}
                    onSwipe={(action) => runReadAction(view, action === "read")}
                  >
                    {chatButton}
                  </ChatSwipeRow>
                ) : (
                  chatButton
                )}
                {!mobile && (
                  <button
                    class="chat-row-more"
                    type="button"
                    aria-label={`Actions for ${displayName}`}
                    aria-haspopup="menu"
                    aria-expanded={menuView === view}
                    aria-controls={menuView === view ? menuId : undefined}
                    onContextMenu={(event) => rowEvents.onContextMenu(event)}
                    onClick={(event) =>
                      chatMenu.openFromButton(
                        view,
                        event.currentTarget,
                        !event.detail,
                      )
                    }
                    onKeyDown={(event) => {
                      if (
                        event.key !== "ArrowDown" &&
                        event.key !== "ContextMenu" &&
                        !(event.shiftKey && event.key === "F10")
                      )
                        return;
                      event.preventDefault();
                      chatMenu.openFromButton(view, event.currentTarget, true);
                    }}
                  >
                    <svg viewBox="0 0 16 16" aria-hidden="true">
                      <path d="m4 6 4 4 4-4" />
                    </svg>
                  </button>
                )}
              </li>
            );
          })}
        </ol>
        {!conversations.length && (
          <p class="no-rooms">{mobile ? "No chats" : "No conversations"}</p>
        )}
      </nav>
      <p
        role="status"
        class={readStatus?.error ? "no-rooms" : "visually-hidden"}
      >
        {readStatus?.text}
      </p>
      {menuView && chatMenu.menu && (
        <MessageActionMenu
          className={mobile ? "" : "chat-action-menu"}
          id={menuId}
          label="Chat actions"
          position={chatMenu.menu.position}
          trigger={chatMenu.trigger}
          onClose={chatMenu.close}
          actions={[
            {
              label: pinned ? "Remove from pinned" : "Pin Chat",
              accessibleLabel: pinned ? "Remove from pinned" : "Pin Chat",
              icon: <ChatActionIcon kind="pin" />,
              disabled: false,
              run: () => preferences.change("pinned", menuView, !pinned),
            },
            {
              label: "Mark as read",
              accessibleLabel: "Mark as read",
              icon: <ChatActionIcon kind="read" />,
              disabled: !marked && !menuUnread,
              run: () => runReadAction(menuView, true),
            },
            {
              label: "Mark as unread",
              accessibleLabel: "Mark as unread",
              icon: <ChatActionIcon kind="unread" />,
              disabled: marked || menuUnread,
              run: () => runReadAction(menuView, false),
            },
          ]}
        />
      )}
      <div class="sidebar-bottom">
        <div class="profile">
          <UserCardTrigger name={state.username} avatar>
            <Avatar
              url={state.avatars?.[state.username]}
              name={state.username}
            />
          </UserCardTrigger>
          <div class="profile-name">
            <strong>
              <DisplayName name={state.username} interactive />
            </strong>
            <small>
              {state.groups.includes("su")
                ? "su"
                : state.groups.join(", ") || "user"}
            </small>
          </div>
          <button
            class="settings-toggle"
            onClick={openSettings}
            aria-label="Settings"
            title="Settings"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="m9 3-.6 2.2-1.7 1L4.5 6l-2 3.5L4 11v2l-1.5 1.5 2 3.5 2.2-.2 1.7 1L9 21h4l.6-2.2 1.7-1 2.2.2 2-3.5L18 13v-2l1.5-1.5-2-3.5-2.2.2-1.7-1L13 3H9Z" />
              <circle cx="11" cy="12" r="3" />
            </svg>
          </button>
          <button
            class="notification-toggle"
            onClick={toggleNotifications}
            aria-pressed={notifications}
            aria-label={
              notifications
                ? "Mute message notifications"
                : "Enable message notifications"
            }
            title={
              notifications
                ? "Room mention and private message notifications on"
                : "Room mention and private message notifications muted · Click to enable"
            }
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M5 17h14l-2-3V9a5 5 0 0 0-10 0v5l-2 3Zm5 3h4" />
              {!notifications && <path d="m3 3 18 18" />}
            </svg>
          </button>
          <button
            class="signout"
            onClick={logout}
            aria-label="Sign out"
            title="Sign out"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M10 5H5v14h5M9 12h11m-4-4 4 4-4 4" />
            </svg>
          </button>
        </div>
      </div>
    </aside>
  );
}
