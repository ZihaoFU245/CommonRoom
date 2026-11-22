import { UserCardTrigger } from "./UserCardTrigger.tsx";
import { DisplayName } from "./DisplayName.tsx";
import { Avatar } from "./Avatar.tsx";
import type { Snapshot } from "../api/protocol.ts";
import type { RefObject } from "preact";
import { Brand } from "./Brand.tsx";
import { UnreadBadge } from "./UnreadBadge.tsx";
import { conversationList } from "../features/conversation/conversationList.ts";
import { reorderMotion } from "../features/preferences/motion.ts";
import autoAnimate from "@formkit/auto-animate";
import { useEffect, useMemo, useRef } from "preact/hooks";
interface Props {
  mobile: boolean;
  heading: RefObject<HTMLHeadingElement>;
  state: Snapshot;
  selected: string;
  peers: string[];
  drafts: string[];
  choose: (name: string) => void;
  openSettings: () => void;
  toggleNotifications: () => void;
  notifications: boolean;
  logout: () => void;
}
export function Sidebar({
  mobile,
  heading,
  state,
  selected,
  peers,
  drafts,
  choose,
  openSettings,
  toggleNotifications,
  notifications,
  logout,
}: Props) {
  const conversations = useMemo(
    () => conversationList(state.rooms, peers, drafts, state.unread),
    [state.rooms, peers, drafts, state.unread],
  );
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!list.current) return;
    const animation = autoAnimate(list.current, reorderMotion);
    return () => animation.destroy?.();
  }, []);
  return (
    <aside id="navigation" class="sidebar visible">
      <div class="sidebar-heading">
        {mobile ? (
          <h1 ref={heading} tabIndex={-1}>
            Conversations
          </h1>
        ) : (
          <Brand />
        )}
      </div>
      <nav class="navigation" aria-label="Conversations">
        <div class="conversation-list" ref={list}>
          {conversations.map(({ view, kind, name }) => {
            const unread = state.unread[view]?.count ?? 0;
            return (
              <button
                key={view}
                type="button"
                class={`room-link ${selected === view ? "selected" : ""} ${unread > 0 ? "unread" : ""}`}
                onClick={() => choose(view)}
              >
                {kind === "direct" ? (
                  <>
                    <span class="private-chat-icon">
                      <span class="hash" aria-hidden="true">
                        ↗
                      </span>
                      <Avatar url={state.avatars?.[name]} name={name} />
                    </span>
                    <DisplayName name={name} />
                  </>
                ) : (
                  <>
                    <span class="hash">
                      {view.startsWith("@private:") ? "↗" : "#"}
                    </span>
                    {name}
                  </>
                )}
                <UnreadBadge count={unread} />
              </button>
            );
          })}
        </div>
        {!conversations.length && <p class="no-rooms">No conversations</p>}
      </nav>
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
