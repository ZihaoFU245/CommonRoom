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
}: {
  consoleView: boolean;
  direct: boolean;
  peer: string | null;
  selected: string;
  online: string[];
  username: string;
  status: string;
}) {
  return (
    <header class="chat-header">
      <div class="room-title">
        <span>{consoleView ? ">" : direct ? "↗" : "#"}</span>
        <h1>
          {consoleView ? (
            "Command"
          ) : direct ? (
            peer ? (
              <DisplayName name={peer} />
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
  );
}
