# Commands

Type commands in the composer. `/help` lists commands permitted here; `/man user`,
`/man room`, and `/man grant` explain syntax and authorization. Commands check both
the selected context and the target. Output stays in this view and tab until reload.
Passwords are masked. Enter sends, Shift+Enter adds a line, and Tab completes.

## Users

```text
/user ls [scope]
/user get who [key] [scope]
/user set who key value [scope]
/user rename [who] new-name
/user new name password [user|admin]
/user delete who
/user disable who
/user enable who
/user passwd old-password new-password
/user reset who key|@all [scope]
/user reset who password new-password
```

Omitted scope uses the selected room; Console/stdin use `@global`.
In private views, specify a room or `@global` for user lists/metadata. `#support` names a room explicitly. `/user ls` lists its members using `r:member.list`;
global listing requires `r:account.list`. `/user rename robert` renames yourself.
Names use 1–32 Unicode characters without whitespace, controls, or ASCII
punctuation except `_` and `-`.

Each account has a global root and sparse room branches. `/user get bob #support`
(or `/user get bob` while in that room) displays a `#support` section and, with
account/global read authority, an `@global` section. Global entries show values without hints. Room entries show `set here` or
`inherited`. Room overrides take precedence; `@unset`
removes the override and restores inheritance. Custom keys inherit too, but global
custom values appear only with permission to read that account's global metadata.
Room managers without that permission see local custom values and effective media/cleanup
limits. `media.user_quota` appears only in the global section and is never a room
override. `/user get @all #support` applies the same view to every member.
Metadata never grants permissions. Only the public global avatar link is projected in broadcast snapshots.
Read your own metadata;
other users require `r:account.metadata` at global/account scope or room policy
management. Only su can set, remove or bulk-update metadata. Global admins
receive `r:account.metadata` for reading; room policy managers may inspect their
members' room metadata. `x:account.metadata` remains reserved for su, and an
explicit write flag does not bypass this restriction. Stdin and authorized
`/sudo` commands act with su authority.

```text
/user set bob media.max_size 10m #support
/user set @all media.max_age 7d #support
/user set bob media.user_quota 100m @global
/user set bob label reviewer
/user set bob label @unset
```

Keys use 1–64 lowercase letters, digits, `.`, `_`, or `-`. Values are single tokens
up to 256 UTF-8 bytes without controls; each scope holds at most 32 keys. `@unset`
removes a key. `media.max_size` means bytes per file, `media.max_age` accepts
s/m/h/d/w durations or `-1` for permanent, and `media.user_quota` is a global
account byte quota. Positive size/quota values cannot exceed server ceilings.
Room media values override global values, then server defaults; getters show
effective media values, including inherited values and server defaults. Only su may change limits, within the server ceilings.
Unknown `media.*` keys are rejected. Custom keys never change permissions. `/user get bob @all` reads all keys;
`/user get @all @all` reads all users and keys in the selected scope.

Account rename preserves identity, metadata, history and cursors. Deletion
revokes sessions and removes private conversations, retaining room messages as
`name (deleted)`. Reusing a name inherits no old data. Ordinary admins cannot
delete themselves or the last active admin; password resets need explicit rights.

## Rooms

```text
/room ls
/room new #name
/room delete #name
/room rename #name new-name
/room join #name
/room leave [#name]
/room add who [#name]
/room kick who [#name]
/room owner who [#name]
```

Explicit room scopes require `#name`. Listing shows global rooms you can discover/read.
New rooms are invitation-only, with ownership and participant/management grants
assigned to the creator. Each operation requires its matching action grant. `/room add bob` invites Bob
to the selected room; `/room add bob #support` names a target explicitly.
`add`, `kick`, `owner` and `leave` all allow the room argument to be omitted.
Ordinary joins open authorized rooms; su and sudo joins also save membership.
Owners must transfer before leaving or being removed. Rename preserves identity,
scoped metadata, history, grants and cursors. Deletion removes room metadata;
reusing the name starts fresh. Browser deletion asks for confirmation before sending.

## Grants and bulk selection

```text
/grant user|@all group [#room|@global]
/grant user|@all scope permission
/revoke user|@all group [#room|@global]
/revoke user|@all scope permission
```

`@all` selects every active account globally, or all active members of the named
room. Grant/revoke retain explicit direct-permission scopes; group assignments
without a room remain global; explicit `@global` is equivalent. A room group grant
that admits a new member also requires `x:member.add` and participant delegation
authority. For example `/grant @all #support w:media.upload` restores room uploads and
`/revoke @all #support /reply` removes direct reply-command grants. Existing group
rights still apply. All targets are authorized against the original policy;
any rejection or failed database write rolls back the complete batch. Protected
su accounts, ownership, delegation and last-active-admin checks still apply.
Metadata `@all` similarly includes every account/member in its scope (including
disabled accounts). No selector silently skips protected users.
`media.user_quota` always writes the global root. Its omitted-scope shorthand keeps
`@all` selection in the current view: `/user set @all media.user_quota 128m` in a
room updates only that room's members. Add `@global` explicitly to select all accounts.
The same rule applies to quota reset.

Grant/revoke only change access. Upload size, retention and byte quota belong in
`/user set`; extra media arguments on `/grant` are rejected. Cleanup's existing
limits also belong in metadata through `history.clean_min_age`. See `/man permissions`,
`/man groups`, and `/man scopes` for action/command grants and predefined bundles.

## Other commands and browser aliases

Message commands (`/tell`, `/history`, `/reply`, `/react`, `/edit`, `/retract`),
`/permissions`, `/sudo`, `/bot`, `/configs`, `/clean`, `/debug`, `/whoami`,
`/console`, `/clear`, `/logout`, `/help` and `/man` retain their purposes.
`/clean age scope [--dry-run]` always requires an explicit scope. Use `#ROOM`,
`@direct:USER` for your existing private chat, `@private:a:b` for one pair,
`@private` for all private chats, or `@global` for all conversations. `@all` is
an explicit global alias. Both `/clean` and `x:history.clean` must allow every
target, including private chats; participants have no cleanup permission by
default. Su may delegate private cleanup with explicit command and action grants.
`--dry-run` checks access and counts messages/references without database writes.
Execution with no matching messages makes no writes or revision/audit changes.
Execution deletes matching history atomically; any denied target or failed
storage write rejects the whole operation. Reports distinguish message/reference
removal from later storage cleanup and quota release. Shared media remains while
retained messages reference it. See `/man clean` for examples. Configuration changes require a restart.

`/alias rooms /room ls` saves a browser command prefix. Expansion happens once,
preserves arguments, and works inside `/sudo`; it never adds permissions.
Settings → Alias manages mappings in localStorage, shared by accounts in that
browser profile. `/alias` and `/sudo` cannot be remapped. Deletes and retractions,
including aliases and sudo, ask for browser confirmation; stdin acts directly.

## Messages and settings

Private conversations appear after exchanging messages. Start one with
`/tell USER MESSAGE`; ordinary text in that private view sends to the peer.
Opening a conversation loads a 100-message page; scroll to load older/newer pages. Each browser conversation has a sliding 1,000-message cache and only visible rows are mounted. `/history 1000 1099` prints messages whose global sequences are in that inclusive range, without fetching earlier messages. Use `/history #study` to read the latest 100 messages from another authorized room without selecting it. Explicit room names require `#`. `/history` reads the latest 100 messages in the current conversation, including a selected private chat. Ranges apply only to the current conversation. A wide range returns at most 100 messages; continue after the last printed sequence. The former count/user syntax is removed.

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
The bell shows a slash while muted. Click the enabled bell to mute immediately;
click the muted bell to enable notifications. Blocked permission is reported
with instructions to change the browser's site settings.
Composer suggestions show one level at a time: `/u` offers `/user`,
`/user ` offers subcommands, and `/user ls ` offers scopes. Free-form arguments
show no fallback wall of syntax; use `/man` for usage and permission details.
All matching suggestions remain available in a scrollable list. Up/Down selects
a match, Tab completes it, and repeated Tab cycles argument matches. Choosing a command or subcommand
advances to its next level. Shift+Tab moves
focus backward. Completion replaces the token at the cursor and preserves
following text. Command arguments suggest known users, rooms, scopes, permissions,
retained message IDs/reactions, and bot subcommands. New names, passwords, bot
token IDs remain free-form.

Connected bots advertise their own message commands through Bot API v1. Type
`@BOT /` to see that bot's commands in the current conversation. Structured
subcommands complete one level at a time: `@SimpleBot /config ` offers `get` and `set`; choosing a mode shows its arguments.
These messages are delivered to the bot's handler. The browser uses cached
metadata and makes no request while you type. See [Bots](bots.md#command-discovery).
The transcript displays the mention followed by the complete command and
arguments as literal code, including when Markdown is enabled. The invocation
is an ordinary room message visible to all authorized readers.

Settings opens Account with `/user rename` and `/user passwd` forms. Appearance controls
light/dark/system/custom palettes, fonts, chat text (14–24px), and UI text
(12–18px); preferences stay in this browser. Reset restores defaults. Theme
applies before rendering, and palette text contrast is at least 4.5:1.
Preferences provides Enable Markdown (the same sending preference as
`/markdown on|off`) and Enable LaTeX math. Both are saved in this browser.
The Markdown checkbox affects future messages; each stored message retains
its existing format. The LaTeX checkbox immediately changes how Markdown
messages are displayed, including history, without changing plain-text messages.

Navigation remembers its open or closed state in this browser across refreshes.
The menu button moves to the right of the logo and becomes an X while navigation
is open. AutoAnimate gives
opening and closing a short transition, respecting reduced-motion preferences.
Live messages rise into the conversation while existing messages move up;
retractions fade away while surrounding messages slide into place. Edits fade
in their updated text and smoothly reposition neighboring rows. These short
180 ms ease-out transitions match the inline editor, respect reduced motion,
and preserve the reading position when browsing older history. Loading history
and switching conversations do not replay message animations.
The message input stays centered in the conversation pane and stops widening
on large windows. It grows and shrinks with wrapped text and new lines, up to
240px or 40% of the viewport height; longer drafts scroll inside the input with
the scrollbar hidden. The online count opens a short animated dropdown; click
outside or press Escape to close it.

## Bot management

Lifecycle and credential subcommands require global admins, su or stdin. `/bot new name` defaults to the selected room; Console/stdin default to `@global`; private views require an explicit room or `@global`. Creation grants an explicit safe participant set, without the ordinary user bundle's settings/retraction privileges. `/bot ls [scope]` and `/bot info name [scope]` use the same scope rules. `/bot rm name @global` deletes the account and credentials; use /room kick to remove room participation.

Credentials are account-wide: `/bot token new name label` shows a secret once, `/bot token ls name` lists IDs/labels/creation times, and `/bot token rm name id` revokes one. Token management requires x:bot.credentials and respects protected-su targets. Use /man bot or [Bots](bots.md) for the full contract. Configure workers using their advertised chat commands, such as `@helper /config get` and `@helper /config set key "value"`. The worker enforces its owner/admin policy; /bot does not launch agents.

## Markdown messages

In the web composer, `/markdown on` formats future room messages, private
messages (`/tell`) and replies with Markdown. `/markdown off` restores plain
text (the default). `/man markdown` explains the feature; completion suggests
`on` and `off`. The preference stays in this browser and does not grant any
message permissions. Each message retains its chosen format for recipients
and history, independently of later toggle changes. Supported formatting includes
bold, italic, strikethrough, headings, lists, quotes, code, tables and links.
Enable LaTeX math in Settings → Preferences to render inline `$...$` / `\(...\)`
and display `$$...$$` / `\[...\]` equations with KaTeX, including fractions,
integrals, sums and matrices. It defaults off, works without HTML or scripts,
and leaves code spans/blocks literal. Invalid math displays a readable fallback.
This supports LaTeX math notation rather than full LaTeX documents.
Markdown images and GIFs use `![alt](url)` and render without a separate opt-in.
Only absolute HTTP/HTTPS URLs, canonical root-relative attachment paths, and base64 raster images (PNG, JPEG, GIF, WebP,
AVIF, BMP) are allowed. Rejected URLs display the alt text. Images are lazy,
use `no-referrer`, and fit the message width. Remote hosts still receive the
viewer's IP address when an image loads.

HTML displays as text when Settings → Security → **Allow HTML** is off.
This single option defaults off and is saved in this browser as
`commonroom-markdown-html`. It offers a per-message Render HTML button for all
HTML, CSS and JavaScript, including passive markup. Every preview uses the same
`markdown-frame.html` opaque-origin sandbox and requires an explicit click.
In mixed messages, surrounding Markdown renders normally; only HTML portions
appear in compact Show text / Render HTML covers. Adjacent HTML, CSS and scripts
share a single preview. Each portion has an independent rendering choice. Show text toggles a
scrollable source panel capped at 400px; selecting it again folds the panel.
With Allow HTML off, the bounded source panel appears directly. Panel transitions
respect reduced-motion preferences. Show text also switches a rendered preview
back to its source. Editing the message resets every portion’s choice.

Full HTML documents preserve indentation instead of becoming Markdown code.
Fences, inline code, autolinks and Markdown images do not trigger HTML previews.
Code previews allow inline CSS/JavaScript and data images/media/fonts, but block
external dependencies/network fetches, forms, popups, parent access and top
navigation. Preview code cannot read chat storage, cookies or the parent DOM.
Choices survive virtual scrolling within the current bounded conversation window;
edits, changing Allow HTML, leaving the conversation, eviction and refresh
reset them. A remounted frame restarts the app/game using the cached frame shell.
Failed loads keep source visible and provide a status. These preferences do not
change stored messages or other recipients. The basic renderer and optional HTML,
TeX, KaTeX and frame dependencies load only when their rendering path is needed.

## Editing messages

`/edit message-id message` updates a retained room or private message.
It requires the `/edit` command grant, `r:message.read`, and
`w:message.edit.own` for your own messages or `w:message.edit.any` for any
author, on the selected conversation. Ordinary room members and
private participants receive editing rights through their participant bundle.
Explicit grants, including bot grants, must include editing separately.
Ordinary admins and room owners do not receive edit-any by default. Superusers
and the stdin console have edit-any; other accounts need explicit scoped grants.

The Edit button appears between Reply and Delete on messages you have permission to edit. The
inline editor offers Save and Cancel; Escape cancels. Failed saves retain the
draft for retry. The editor fits its full text and grows or shrinks as you type
or resize the view. Edits support nonempty text of at most 4,000 Unicode characters.
They preserve message identity, original time, sequence, Markdown format,
reactions, and existing reply quotes, and recalculate mentions. An `edited`
boolean marks edited messages; absent values default to false. Changes persist
and are broadcast only to authorized conversation readers. Editing does not
create a new message or advance read cursors.

Saving unchanged text behaves like Cancel: it does not send an edit or add an
edited marker. Existing edited markers from earlier changes remain. Direct
`/edit` commands with unchanged text also leave storage and revisions unchanged.

`/bot owner BOT USER` assigns bot ownership by stable account identity; `/bot info`
shows it. These remain server management commands. A bot may define its own
`@BOT /command` controls and restrict them to its owner/administrators. simpleBot's
configuration/diagnostic controls use that policy and are hidden from other
users' completion. The model instead receives native server commands allowed by
the bot account's grants.

When the optional SimpleBot demo is installed, regular members can use `@SimpleBot /help` and `@SimpleBot /ping` without a model call. Its owners/admins can use `@SimpleBot /config get` to list all settings
(secrets redacted), `/config set key "value"` to apply a quoted value immediately,
and `@SimpleBot /reload` to validate and load file edits. `prompt` configures
personality and user preferences as a separate user message; the system prompt
remains fixed in code. These are worker controls; `/bot` remains server management.


SimpleBot optionally supports autonomous group participation. An owner/admin can
use `@SimpleBot /config set autonomous "true"` to consider unmentioned room
messages in fixed three-second batches. Mentions can accelerate a batch; model continuations
and tool completions can trigger further ticks without another mention. Rooms
and DMs share the event-driven tick loop documented in
[SimpleBot](../simpleBot/README.md#async-event-loop-and-model-ticks). Related questions can share one plain
message; independent questions can receive separate targeted replies in one
turn, or the bot can stay silent. This changes worker behavior within existing
server grants. Set `autonomous` to `"false"` to restore mention-only room input.
`model.maxToolCalls` defaults to 32 (range 1–128); explicitly update older saved
limits with `@SimpleBot /config set model.maxToolCalls "32"`.

Media sizes and account quotas use metadata. `k`, `m`, `g` mean powers of
1024 (case-insensitive): `512k`, `25m`, `1g`. Config media size/quota fields
use the same string notation. `/user get` lists effective media limits even
when no override is stored; global custom metadata requires account read authority.

Room members can upload by default. Owners can block a member's uploads with
`/revoke user #room w:media.upload`, and restore them with `/grant user #room w:media.upload`. This room restriction overrides upload grants there (except su),
while leaving text and read permissions unchanged.

## Command output formatting

All command replies use the same Markdown display, independently of the message
Markdown preference. Help groups commands by category and lists each full
subcommand on its own line. Manuals support headings, nested and numbered lists,
tables, emphasis, links and language-labelled code fences.

User/room lists and bot details receive headings and readable lists. Configuration appears as formatted JSON; user
metadata appears as per-user scope sections with value/source lists; history and credentials
remain literal code blocks. Errors and ordinary acknowledgements preserve their
text without interpreting user-provided Markdown. HTML stays text, images do not
load, and command output never opens an HTML or attachment preview. Code blocks
and tables scroll within the output on narrow screens; inline syntax wraps.

Only presentation changes for structured results: wire-level JSON, history and
credential text remain intact for API consumers and copy/paste.

List headings identify their scope: `/user ls #support` starts with **Users in
#support**; `/user ls` uses the originating room, and global lists say
**Users globally**. Metadata headings identify the target users and scope.
Manual explanations use short Markdown bullets under headings, with separate
syntax/example code blocks. Errors use the public `/user` and `/room` syntax.

Scope defaults apply to user lists/metadata, bot new/list/info,
permissions and history. Cleanup requires an explicit scope, including in
Console, rooms and private views. Room discovery, account lifecycle, bot credentials and
server configuration remain inherently global operations. Room membership
commands use the selected room and require a named room in Console/private views.

`/user set bob media.user_quota 1g` updates global account quota when scope is
omitted; explicit room scopes cannot hold account-wide quota. For bulk quota
updates, `@all` still selects members of the current room (all accounts in
Console). Only su can execute these metadata updates.

Cleanup limits use `/user set who history.clean_min_age value [scope]` (su only).
`0` disables cleanup, `-1` removes the age restriction, and positive durations
(`7d`, `30d`) set the minimum age of deletable messages. Room overrides win over
global values. Missing values derive from current roles: admin/su `-1`, ordinary
users `0`, and room owners `-1` in their own room. Private cleanup uses the global
value. The wire result retains `role` as the source of a derived default; the UI labels
room values supplied outside the displayed section as `inherited`; global
entries have no hint.

`/clean -1 #support` explicitly wipes all history in that scope. Both age and
scope remain required. Su and authorized `/sudo` bypass command/action grants
and cleanup metadata limits. Other callers need both grants and an allowing
metadata limit. Bulk cleanup authorizes every target and its limit before deleting.
`/grant` and `/revoke` accept no limit arguments.

`/user reset bob media.max_size #support` removes that room override and restores
inheritance. `/user reset bob @all #support` clears every override in that room
branch; `/user reset bob @all @global` clears only the global root. Other branches
remain unchanged. `@all` may also select users, as in
`/user reset @all @all #support`. Metadata resets are su-only, use the same scope
and bulk selection rules as `/user set`, and prune empty branches. Account-wide
quota reset defaults to `@global` even from a room.

Password reset uses `/user reset bob password new-password`; it targets one
account, accepts no scope, and retains `x:account.password.reset` authorization.
The literal `password` selector separates credential resets from metadata resets.
Password changes keep the existing session-revocation and post-hash rechecks.

## Avatar settings

Settings → Account shows your round avatar, name and groups. Click the avatar
(or the plus placeholder) for Upload, Set URL and Remove. HTTPS image URLs such
as Gravatar work without object storage. Upload accepts PNG, JPEG or WebP up to
20 MiB/40 megapixels; position and zoom the circular preview, then confirm. The
browser crops the selected square into a 512×512 PNG (maximum 2 MiB) before
uploading to S3. Cancel leaves the existing avatar unchanged. Avatars appear next
to chat authors and sidebar names only after loading successfully.
