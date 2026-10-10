import type { Snapshot } from "../api/protocol.ts";
import { Brand } from "./Brand.tsx";
import { UnreadBadge } from "./UnreadBadge.tsx";
interface Props {
  state: Snapshot;
  menu: boolean;
  selected: string;
  peers: string[];
  peer: string | null;
  closeMenu: () => void;
  choose: (name: string) => void;
  openSettings: () => void;
  toggleNotifications: () => void;
  notifications: boolean;
  logout: () => void;
}
export function Sidebar({
  state,
  menu,
  selected,
  peers,
  peer,
  closeMenu,
  choose,
  openSettings,
  toggleNotifications,
  notifications,
  logout,
}: Props) {
  return (
    <aside id="navigation" class={`sidebar ${menu ? "visible" : ""}`}>
      <div class="sidebar-heading">
        <Brand />
        <button
          class="sidebar-close"
          aria-label="Close navigation"
          onClick={closeMenu}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="m6 6 12 12M18 6 6 18" />
          </svg>
        </button>
      </div>
      <nav class="navigation" aria-label="Conversations">
        <details class="nav-group" open>
          <summary>
            Rooms<span class="group-count">{state.rooms.length}</span>
          </summary>
          <div class="nav-items">
            {state.rooms.map((r) => (
              <button
                key={r.name}
                class={`room-link ${selected === r.name ? "selected" : ""}`}
                onClick={() => choose(r.name)}
              >
                <span class="hash">
                  {r.name.startsWith("@private:") ? "↗" : "#"}
                </span>
                {r.name.startsWith("@private:") ? r.name.slice(9) : r.name}
                <UnreadBadge count={state.unread[r.name]?.count ?? 0} />
              </button>
            ))}
            {!state.rooms.length && <p class="no-rooms">No rooms</p>}
          </div>
        </details>
        <details class="nav-group" open>
          <summary>
            Private messages<span class="group-count">{peers.length}</span>
          </summary>
          <div class="nav-items">
            {peers.map((name) => (
              <button
                key={name}
                class={`room-link ${peer === name ? "selected" : ""}`}
                onClick={() => choose(`@direct:${name}`)}
              >
                <span class="hash">↗</span>
                {name}
                <UnreadBadge
                  count={state.unread[`@direct:${name}`]?.count ?? 0}
                />
              </button>
            ))}
            {!peers.length && <p class="no-rooms">No conversations</p>}
          </div>
        </details>
      </nav>
      <div class="sidebar-bottom">
        <div class="profile">
          <div class="profile-name">
            <strong>{state.username}</strong>
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
                ? "Disable mention notifications"
                : "Enable mention notifications"
            }
            title={
              notifications
                ? "Mention notifications on"
                : "Enable mention notifications"
            }
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <path d="M5 17h14l-2-3V9a5 5 0 0 0-10 0v5l-2 3Zm5 3h4" />
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
