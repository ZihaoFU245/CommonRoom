import { DisplayName } from "./DisplayName.tsx";
import { OnlineUsers } from "./OnlineUsers.tsx";
import { Avatar } from "./Avatar.tsx";
import type { RefObject } from "preact";
export function ChatHeader({
  heading,
  onBack,
  consoleView,
  direct,
  peer,
  avatar,
  selected,
  online,
  username,
  status,
}: {
  heading: RefObject<HTMLHeadingElement>;
  onBack: (() => void) | undefined;
  consoleView: boolean;
  direct: boolean;
  peer: string | null;
  avatar: string | undefined;
  selected: string;
  online: string[];
  username: string;
  status: string;
}) {
  return (
    <header class="chat-header">
      {onBack && (
        <button
          type="button"
          class="conversation-back"
          aria-label="Back to conversations"
          onClick={onBack}
        >
          <svg viewBox="0 0 24 24" aria-hidden="true">
            <path d="m14 6-6 6 6 6" />
          </svg>
        </button>
      )}
      <div class="room-title">
        {!consoleView && direct && peer ? (
          <span class="private-chat-icon">
            <span class="hash" aria-hidden="true">
              ↗
            </span>
            <Avatar url={avatar} name={peer} />
          </span>
        ) : (
          <span>{consoleView ? ">" : direct ? "↗" : "#"}</span>
        )}
        <h1 ref={heading} tabIndex={-1}>
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
