# Commands

## Using commands

Type commands in the composer. Enter sends, Shift + Enter adds a line, Tab accepts
a suggestion, arrow keys choose one, and Escape closes the suggestions.
`/help` lists commands permitted in the current context; `/man COMMAND` explains
server command syntax and required grants. The server checks both the invocation context and
the target. Opening `/console` adds no permissions. Accounts without conversations
start in Command; select a conversation or use `/join ROOM` to return.

Commands and their replies stay in the originating view and tab. Reloading clears
command output and reloads server history. `/clear` only clears the local display.
Passwords are masked in local output and never logged. The URL remembers the
selected conversation. Memberships and private peers come from the server on
login, reconnect and foreground refresh.

## Syntax

| Command | Who | Behavior |
| --- | --- | --- |
| `/ls [room\|user\|group\|@user\|@admin\|@su]` | Matching list grants | Default: rooms and own readable private pairs; user lists accounts; @group filters global assignments; group lists predefined bundles |
| `/new room NAME` or `/new user NAME PASSWORD [user\|admin]` | Room/account-create grant | Create an invitation-only room or account; admin assignment needs its own grant |
| `/rm room NAME` or `/rm user NAME` | Room/account-delete grant | Permanently remove the target; web asks for confirmation |
| `/rm group USER GROUP [ROOM]` | Group management grants | Remove an assignment; other assignments and direct grants still apply |
| `/alias A B` | Web only | Replace one command prefix; save the mapping in this browser |
| `/help` | Authorized commands | Show commands permitted in the current context |
| `/man [command\|topic]` | Manual command grant | Read manuals: grant, revoke, permissions, groups, scopes, ownership, or any server command |
| `/whoami` | Accounts/stdin | Show name and group label (`user`, `admin`, `su`) |
| `/console` | Web command grant | Open the Command view in this tab; does not add permissions |
| `/sudo /command [arguments]` | Global `/sudo` and `x:command.sudo` grants | Execute one command with su authority while retaining the caller's identity |
| `/permissions [scope]` | Accounts/stdin | Inspect effective grants; `@groups` lists predefined bundles, `@audit` requires su |
| `/grant user group [room]` or `/grant user scope permission [minimum-age]` | Scoped grant/group managers | Assign a predefined group or add a direct action/command grant; `/man grant` explains |
| `/revoke user group [room]` or `/revoke user scope permission` | Scoped grant/group managers | Remove a group assignment or direct permission; `/man revoke` explains |
| `/owner user [room]` | Room management grants | Transfer room ownership |
| `/debug on\|off` | Metadata grant | Toggle expandable details for readable messages in this tab |
| `/rename [user] new-name` | Own/any rename grant | Rename yourself, or an authorized target; preserve identity, history, sessions and read positions |
| `/passwd old new` | Own-password grant | Change password and sign out other sessions |
| `/members [room]` | Member-list grant | List authorized conversation participants |
| `/history [start end] [view]` | Read and command grants | Read inclusive sequence range; default latest 50, maximum 100 per page |
| `/join room` | Read/join grants | Open an authorized room; does not create membership |
| `/leave [room]` | Accounts | Remove own room access; owners must transfer first |
| `/tell user message` | Private-send grants | Send within a private conversation |
| `/react message-id reaction` | Read/reaction grants | Toggle a reaction on a message in the selected conversation |
| `/retract message-id` | Read/retract grants | Retract own messages; su has permission to retract any message |
| `/reply message-id message` | Read/write grants | Reply within the selected conversation |
| `/add user [room]` | Room-invite grant | Add participant with room user grants |
| `/kick user [room]` | Room-remove grant | Remove participant and direct room grants |
| `/configs` | Configuration grant | Print active configuration; changes require restart |
| `/clean age [scope]` | Scoped cleanup grants | Authorize every target before deleting old messages; units s/m/h/d/w |
| `/clear` | Web command grant | Clear visible local history; refresh restores persisted history |
| `/logout` | Web | Sign out |
| `/reset name password` | Explicit password-reset grant | Reset password and revoke sessions; ordinary admins have no default reset grant |
| `/disable name` | Account-disable grant | Disable, revoke sessions and remove room access |
| `/enable name` | Account-enable grant | Enable without restoring room access |

## Browser aliases

`/alias rooms /ls room` makes `/rooms` run `/ls room`. A accepts a command name
with an optional leading slash; B accepts a command and subcommand. Repeating A
replaces its mapping. Expansion happens once, preserves following arguments,
and also works inside `/sudo`; it never adds server permissions. `/alias` and
`/sudo` themselves cannot be remapped.

Mappings use localStorage, survive refresh, and are shared by accounts in the
same browser profile. Settings → Alias lets you add, edit, or remove mappings.
Storage failures are reported. Web `/rm` and `/retract`
commands, including aliases and sudo, ask for confirmation before sending.
Stdin removals execute directly. Old command syntax is unsupported; stored-grant
updates are described in [Data](data.md).

## Rooms, accounts and cleanup

New rooms are invitation-only. Creation assigns ownership and grants; `/join`
opens an authorized room without creating membership. There is no default room;
existing rooms, including `lobby`, can be removed. Owners must transfer ownership
before leaving or being kicked. Only su can manage a su account. Ordinary admins
cannot delete themselves or the last active admin.

`/rm user bob` removes Bob's account and private pairs for both participants.
Room messages and reply quotes remain with author `bob (deleted)`. The name and
user-limit slot become available; a replacement account inherits no old access,
DMs, sessions or read positions. Use `/disable` for reversible suspension.
Groups are fixed bundles; `/rm group` removes an assignment, not the bundle.

`/clean 7d ROOM`, `/clean 24h @private`, and `/clean 7d @all` remove messages
older than the specified UTC age (units s/m/h/d/w). Without a scope, cleanup uses
the selected room, or all messages in Command/stdin. It preserves accounts,
rooms and memberships. `/configs` shows active configuration; edit
`data/config.json` while stopped and restart to apply changes. See
[Security](security.md) for delegation, ownership and conditional cleanup grants.

Names contain 1–32 Unicode characters, including Chinese, accents, combining
marks and emoji. Whitespace, controls and ASCII punctuation other than `_` and
`-` are excluded to keep command, mention and conversation syntax unambiguous.

## Messages and settings

Private conversations appear after exchanging messages. Start one with
`/tell USER MESSAGE`; ordinary text in that private view sends to the peer.
Opening a conversation loads a 100-message page; scroll to load older/newer pages. Each browser conversation has a sliding 1,000-message cache and only visible rows are mounted. `/history 1000 1099` prints messages whose global sequences are in that inclusive range, without fetching earlier messages. Add a view, e.g. `/history 1000 1099 @direct:bob`, to target another authorized conversation. A wide range returns at most 100 messages; continue after the last printed sequence. The former count/user syntax is removed.

Messages stay left-aligned, with local dates and times. Consecutive messages
from the same sender group within five minutes; commands break the group.
Unread badges count unread incoming messages. Opening a view starts at the first
unread message; Jump to unread returns there while browsing. Only visible
messages in a focused tab advance the read cursor, which syncs across devices.

Hover actions (always shown on touch devices) provide reply, retract, and
emoji/custom reactions. Replies quote a 160-character preview and jump to the
original when loaded. Retracting clears retained reply previews; expiring a
message preserves them. Reactions accept 1–128 Unicode characters without
controls, with up to 32 reactions per message. `/debug on` shows expandable
metadata only where permitted; it resets on refresh or permission revocation.

`@USERNAME` mentions complete room members or the private peer; email addresses
are excluded. The profile bell enables browser notifications for new mentions.
Notifications require browser permission, work while the app is open, and omit
refresh/history events, own messages, and the focused conversation. Clicking one
opens its conversation; private previews appear only after opting in.

Settings opens Account with `/rename` and `/passwd` forms. Appearance controls
light/dark/system/custom palettes, fonts, chat text (14–24px), and UI text
(12–18px); preferences stay in this browser. Reset restores defaults. Theme
applies before rendering, and palette text contrast is at least 4.5:1.
