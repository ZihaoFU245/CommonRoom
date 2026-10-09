# Commands

## Commands

Admins start in a local Command view and can register or manage accounts there. Rooms and private conversations use foldable navigation lists.
New accounts have no room memberships. Type messages or commands in the composer. Enter sends; Shift + Enter adds
a line. Use `/help` for commands grouped into General, Account, Rooms, Messages, and Server, filtered by your role. Typing `/`
shows contextual command and argument hints; arrow keys select a hint, Tab
completes it, and Escape closes the hints. Enter executes what you typed.
Chat messages use a Discord-style layout, all aligned on the left, with the
sender and local time above the text. Consecutive
messages from one sender are grouped within five minutes; commands break the
group. Dates, times, and date separators follow the browser timezone, with the
full local date and timezone available on timestamp hover. The server stores
Unix timestamps in seconds. Commands and replies retain their console format
alongside chat messages, only in your current tab and the view where the command ran.
The Command view has its own local history. Private messages are grouped by
person; only people you have exchanged private messages with appear in the list. Start a
new private conversation with `/tell person message`; typing in a private conversation is equivalent to `/tell person message`. Reloading clears command output and
fetches a 50-message tail per room and per private conversation. Opening a
conversation fetches its retained history; `/history 200` also prints more
retained messages as local command output. `/clear`
clears the local display without deleting server history; refresh restores it.
The URL remembers the selected room, so refresh returns to the same conversation.
Every permission check happens on the server.

Orange badges count messages from other people that you have not read. Opening
an unread conversation starts at its first unread message; **Jump to unread**
returns to the next unread message while you browse history. Only messages
viewed in a focused, visible conversation advance the read position. Unread
counts sync across your devices and survive restarting or moving `data/`.
Commands and reactions do not add unread messages. Histories evicted by the
retention limit no longer contribute to the count.

The compact top bar shows a green Connected dot and an online count. Open the
count to see connected accounts; a user remains online while any tab or device
is connected.

Message actions appear on hover (always on touch devices). Use **Reply** to
quote a message in the composer, **Delete** to retract your own message for
everyone in the conversation (including its retained reply previews) after
confirmation, or **+** to choose an emoji/custom UTF-8
reaction. The picker opens above when there is insufficient room below in the
visible chat area, and updates its placement on scroll or resize. Clicking a
reaction toggles your participation. Reactions allow
1–128 Unicode characters without control characters, with at most 32 distinct
reactions per message. Reactions and replies are persisted for both rooms and
private conversations; only conversation participants may use them. Quoted
replies keep the original author's name and a 160-character preview even after
the original message expires. The quote jumps to the original if it is loaded.

Type `@username` to mention a room member or the other private-chat participant;
name completion appears as you type. Names are case-sensitive. The server
records mentions only for participants in that conversation; email addresses
are not treated as mentions. Text and custom reactions remain UTF-8.

For desktop Chrome notifications, click the bell next to your profile and
allow notifications in the browser prompt. Production needs HTTPS; localhost
works for development. The setting is remembered per browser origin.
Notifications arrive for new mentions while the page is open and its WebSocket
is connected, when the page is in the background or a different conversation
is selected. Reading the same conversation in the foreground suppresses the
notification. Clicking a notification opens its conversation. Reactions,
refresh, history joins and reconnect snapshots do not replay notifications.
Notifications are not push delivery to a closed browser/tab, and browser/OS
notification settings can block them. Private message previews can appear in
system notifications only after you opt in.

The Settings button beside your profile chooses the theme (Light, Dark, System,
which follows the operating system, or Custom) and adjusts chat text (14–24px)
and UI text (12–18px) independently, taking effect immediately; Reset defaults
restores the theme, the custom colors, and both font sizes. Custom takes a
surface color and an accent color as hex values and generates the rest of the
palette from them: every text, border, hover, and status tone is solved against
a contrast target, and the surface's own lightness decides whether the result is
light or dark. These preferences stay in this browser; they do not change the
server configuration. The theme is resolved from browser storage in the entry
module, before the app renders.
Room and private-chat lists share a scrollable sidebar area; the profile stays
fixed below it. Joined rooms and private peers come from the server on login,
socket reconnect and when the page returns to the foreground. An older HTTP
refresh cannot overwrite a newer WebSocket snapshot.


| Command | Who | Behavior |
| --- | --- | --- |
| `/help` | Everyone | Show the command reference |
| `/whoami` | Everyone | Show your name and permission (`user` or `admin`) |
| `/passwd old new` | Web users | Verify your old password, change it, and sign out other sessions; current session stays logged in |
| `/rooms` | Everyone | List your rooms; admins can discover all rooms |
| `/users` | Everyone | List active account names and permissions |
| `/members [room]` | Everyone | List members of a room you belong to |
| `/history [count] [user]` | Everyone | Read retained messages (default up to 50; per-room and per-private-conversation limit from `max_messages`); private view uses the selected person |
| `/join room` | Everyone | Open a room you already belong to; admins can join any room |
| `/leave [room]` | Everyone | Leave the specified or selected room; an admin must add regular users back |
| `/tell user message` | Everyone | Private message, visible only to sender and recipient |
| `/react message-id reaction` | Web users | Toggle your reaction on a retained message in the selected room or your private history |
| `/retract message-id` | Web users | Delete your own retained room/private message for everyone; remove its retained reply previews |
| `/reply message-id message` | Web users | Reply in the selected room, or to the other participant of a private message |
| `/new room` | Admin or stdin | Create a room; the web admin becomes its first member |
| `/add user [room]` | Admin or stdin | Add an existing account; defaults to the selected room in the web UI |
| `/kick user [room]` | Admin or stdin | Revoke room access immediately |
| `/delete room` | Admin or stdin | Delete room and history |
| `/grant user` | Admin or stdin | Grant administrator permission |
| `/revoke user` | Admin or stdin | Restore user permission; web cannot revoke the last active admin |
| `/configs` | Admin or stdin | Print the active configuration; values require restart to change |
| `/clean age [room\|@private\|@all]` | Admin or stdin | Delete messages older than an age; units `s`, `m`, `h`, `d`, `w` |
| `/clear` | Web only | Clear local console output and visible chat without deleting persisted messages |
| `/logout` | Web only | Sign out |
| `/user name password [admin\|user]` | Admin or stdin | Create an account; default role is user |
| `/reset name password` | Admin or stdin | Reset password and revoke all login sessions |
| `/disable name` | Admin or stdin | Disable account, revoke sessions, and remove memberships |
| `/enable name` | Admin or stdin | Enable account without restoring room memberships; old sessions stay revoked |
| `/deleteuser name` | Admin or stdin | Permanently delete account, sessions, memberships, reactions, read positions, and its private conversations; retain room messages as `name (deleted)` |

`/deleteuser bob` permanently removes Bob's account and private conversations
for both participants. Shared room messages and reply quotes remain, with their
author labeled `bob (deleted)`. The username and user-limit slot become available
again; a replacement account starts without old memberships, DMs, or read
positions. Web admins cannot delete themselves or the last active admin; stdin
can remove any account and create a replacement administrator. Use `/disable`
for a reversible account suspension.

There is no default room. Existing rooms are preserved, including rooms named
`lobby`, which can be deleted like any other room. Rooms are invitation-only for regular users;
admins explicitly control membership. Stdin is the superuser and can manage
all rooms; specify the room on stdin. Account command passwords, including both `/passwd` arguments, are masked in local output and never logged.
`/clean 7d room-name` cleans one room; `/clean 24h @private` cleans private messages;
`/clean 7d @all` cleans all rooms and private messages. Without a scope it uses
the selected room, or all messages in Command and stdin. Cleanup keeps messages
at or newer than the cutoff and reports the number removed. It preserves rooms,
accounts, and memberships. Cleanup is manual and uses stored UTC timestamps.
There is no runtime configuration setter: `/configs` shows the loaded settings,
and editing `data/config.json` takes effect after restart.
Web admins do not receive private
messages between other users. `/tell` opens the private-message view in the UI.
Names contain 1–32 ASCII letters, digits, `_`, or `-`.
