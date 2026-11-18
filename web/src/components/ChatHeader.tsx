import { OnlineUsers } from "./OnlineUsers.tsx";
export function ChatHeader({
  consoleView,
  direct,
  selected,
  online,
  username,
  status,
}: {
  consoleView: boolean;
  direct: boolean;
  selected: string;
  online: string[];
  username: string;
  status: string;
}) {
  return (
    <header class="chat-header">
      {!direct && (
        <div class="room-title chat-header-pill">
          <span>{consoleView ? ">" : "#"}</span>
          <h1>{consoleView ? "Command" : selected}</h1>
        </div>
      )}
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
