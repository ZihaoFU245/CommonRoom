# Commands

## Commands

Accounts without conversations start in a local Command view. Use `/console` to open it from a conversation; select a room or private conversation in the sidebar to return. Account management there requires the appropriate grants. Rooms and private conversations use foldable navigation lists.
New accounts have no room memberships. Type messages or commands in the composer. Enter sends; Shift + Enter adds
a line. Use `/help` for commands grouped into General, Account, Rooms, Messages, and Server, filtered by your grants in the selected context. Typing `/`
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
Every permission check happens on the server. Use `/man` for the manual index,
`/man grant` and `/man revoke` for both command forms, and `/man permissions` for
the complete action catalog. Manuals use headings, short lists, and separate
command examples in the web UI; stdin shows the same structured text.
Manuals are read-only and do not grant execution rights.

`@global` names the global scope (`@server` remains a compatibility alias).
Command suggestions and command-grant confirmations list the additional action
requirements. `/permissions` groups effective read/write/management actions and
command grants separately from the assignments at the selected scope.

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

Message actions appear on hover (always on touch devices). Accounts with metadata grants can use
`/debug on` to reveal a **⋯** button beside **+**; click it to expand selectable
message details, including the message ID. `/debug off` hides the details. Debug
is off after refresh and when metadata permission is revoked; it stays local to the tab. Use **Reply** to
quote a message in the composer, **Delete** to retract your own message for
everyone in the conversation (including its retained reply previews) after
confirmation, or **+** to choose an emoji/custom UTF-8
reaction. The picker opens above when there is insufficient room below in the
visible chat area, and updates its placement on scroll or resize. Clicking a
reaction toggles your participation. Reactions allow
1–128 Unicode characters without control characters, with at most 32 distinct
reactions per message. Reactions and replies are persisted for both rooms and
private conversations; they require matching scoped action and command grants. Quoted
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
which follows the operating system even on the login screen, or Custom) and
adjusts chat text (14–24px) and UI text (12–18px) independently, taking effect
immediately; Reset defaults
restores the theme, the custom colors, and both font sizes. Custom takes a
surface color and an accent color as hex values and generates the rest of the
palette from them: every text, border, hover, and status tone is solved against
a contrast target across paper, hover surfaces, and input fields. Text, including
unread counts, has at least 4.5:1 contrast; surface steps shrink when necessary
to preserve legibility. The surface's own luminance decides whether the result is
light or dark. These preferences stay in this browser; they do not change the
server configuration. The theme is resolved from browser storage in the entry
module, before the app renders.
Room and private-chat lists share a scrollable sidebar area; the profile stays
fixed below it. Joined rooms and private peers come from the server on login,
socket reconnect and when the page returns to the foreground. An older HTTP
refresh cannot overwrite a newer WebSocket snapshot.


| Command | Who | Behavior |
| --- | --- | --- |
| `/help` | Authorized commands | Show commands permitted in the current context |
| `/man [command\|topic]` | Manual command grant | Read manuals: grant, revoke, permissions, groups, scopes, ownership, or any command |
| `/whoami` | Accounts/stdin | Show name and group label (`user`, `admin`, `su`) |
| `/console` | Web command grant | Open the Command view in this tab; does not add permissions |
| `/permissions [scope]` | Accounts/stdin | Inspect effective grants; `@groups` lists predefined bundles, `@audit` requires su |
| `/grant user group [room]` or `/grant user scope permission [minimum-age]` | Scoped grant/group managers | Assign a predefined group or add a direct action/command grant; `/man grant` explains |
| `/revoke user group [room]` or `/revoke user scope permission` | Scoped grant/group managers | Remove a group assignment or direct permission; `/man revoke` explains |
| `/owner user [room]` | Room management grants | Transfer room ownership |
| `/debug on\|off` | Metadata grant | Toggle expandable details for readable messages in this tab |
| `/passwd old new` | Own-password grant | Change password and sign out other sessions |
| `/rooms` | Accounts/stdin | List discoverable rooms with their owners, then your own readable private pairs; one entry per line. Other users' pairs are excluded even for su |
| `/users` | Directory grant | List active accounts and groups |
| `/members [room]` | Member-list grant | List authorized conversation participants |
| `/history [count] [user\|@private:a:b]` | Read and command grants | Read retained history, default up to 50 |
| `/join room` | Read/join grants | Open an authorized room; does not create membership |
| `/leave [room]` | Accounts | Remove own room access; owners must transfer first |
| `/tell user message` | Private-send grants | Send within a private conversation |
| `/react message-id reaction` | Read/reaction grants | Toggle a reaction on a message in the selected conversation |
| `/retract message-id` | Read/retract grants | Retract own messages; su has permission to retract any message |
| `/reply message-id message` | Read/write grants | Reply within the selected conversation |
| `/new room` | Room-create grant | Create an invitation-only room with creator ownership/grants |
| `/add user [room]` | Room-invite grant | Add participant with room user grants |
| `/kick user [room]` | Room-remove grant | Remove participant and direct room grants |
| `/delete room` | Room-delete grant | Delete room, history and scoped grants |
| `/configs` | Configuration grant | Print active configuration; changes require restart |
| `/clean age [scope]` | Scoped cleanup grants | Authorize every target before deleting old messages; units s/m/h/d/w |
| `/clear` | Web command grant | Clear visible local history; refresh restores persisted history |
| `/logout` | Web | Sign out |
| `/user name password [admin\|user]` | Account-create grant | Create account; admin assignment needs its own permission |
| `/reset name password` | Explicit password-reset grant | Reset password and revoke sessions; ordinary admins have no default reset grant |
| `/disable name` | Account-disable grant | Disable, revoke sessions and remove room access |
| `/enable name` | Account-enable grant | Enable without restoring room access |
| `/deleteuser name` | Account-delete grant | Delete account and private pairs; retain room messages with deleted-author label |

See [Security](security.md) for permission names, group bundles, scope syntax,
read-only access, delegation limits, ownership and migration examples.

`/deleteuser bob` permanently removes Bob's account and private conversations
for both participants. Shared room messages and reply quotes remain, with their
author labeled `bob (deleted)`. The username and user-limit slot become available
again; a replacement account starts without old memberships, DMs, or read
positions. Ordinary admins cannot delete themselves or the last active admin. `su` can
manage any account, including other superusers; only `su` can manage a su account. Use `/disable`
for a reversible account suspension.

There is no default room. Existing rooms are preserved, including rooms named
`lobby`, which can be deleted like any other room. Rooms are invitation-only for regular users;
owners and explicitly authorized room managers control membership. Ordinary
admins cannot join someone else’s new private room automatically. Stdin is `su` and can manage
all rooms; specify the room on stdin. Account command passwords, including both `/passwd` arguments, are masked in local output and never logged.
`/clean 7d room-name` cleans one room; `/clean 24h @private` cleans private messages;
`/clean 7d @all` cleans all rooms and private messages. Without a scope it uses
the selected room, or all messages in Command and stdin. Cleanup keeps messages
at or newer than the cutoff and reports the number removed. It preserves rooms,
accounts, and memberships. Cleanup is manual and uses stored UTC timestamps.
There is no runtime configuration setter: `/configs` shows the loaded settings,
and editing `data/config.json` takes effect after restart.
Ordinary web admins do not receive private
messages between other users. Web accounts in `su` can inspect every private pair. `/tell` opens the private-message view in the UI.
Names contain 1–32 ASCII letters, digits, `_`, or `-`.
