import { Avatar } from "./Avatar.tsx";
import { DisplayName } from "./DisplayName.tsx";
import { OnlineUsers } from "./OnlineUsers.tsx";
export function ChatHeader({
  consoleView,
  direct,
  peer,
  selected,
  online,
  username,
  status,
  avatars,
}: {
  consoleView: boolean;
  direct: boolean;
  peer: string | null;
  selected: string;
  online: string[];
  username: string;
  status: string;
  avatars?: Record<string, string>;
}) {
  return (
    <header class="chat-header">
      <div class="room-title chat-header-pill">
        {consoleView ? (
          <span>{">"}</span>
        ) : direct && peer ? (
          <span class="private-chat-icon room-title-avatar">
            <span class="hash" aria-hidden="true">
              ↗
            </span>
            <Avatar url={avatars?.[peer]} name={peer} />
          </span>
        ) : direct ? (
          <span>↗</span>
        ) : (
          <span>#</span>
        )}
        <h1>
          {consoleView ? (
            "Command"
          ) : direct ? (
            peer ? (
              <DisplayName name={peer} interactive />
            ) : (
              "Private messages"
            )
          ) : (
            selected
          )}
        </h1>
      </div>
      <div class="header-actions">
        <OnlineUsers
          names={online}
          username={username}
          connected={status === "online"}
        />
        <span
          class={`connection chat-header-pill ${status}`}
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
  );
}
