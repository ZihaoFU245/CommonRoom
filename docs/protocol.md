# HTTP and WebSocket protocol

All paths are relative to the configured `base_url`. Nginx serves web assets and proxies HTTP API requests and WebSockets to the server's single backend listener. At root hosting the endpoints are:

| Endpoint | Request | Response |
| --- | --- | --- |
| `POST /api/login` | `{username,password}` | `{ok:true}` and HttpOnly session cookie |
| `POST /api/logout` | `{}` | `{ok:true}` and cleared cookie |
| `GET /api/session` | Cookie credential | `{ok:true}` or 401; lightweight reconnect authentication |
| `GET /api/me` | Session cookie | Authorized `Snapshot` |
| `GET /api/history?view=...` | View + sequence cursors/range + limit | `{view,messages,revision,has_before,has_after,name_colors}` |
| `POST /api/read` | `{view,through}` | `{ok:true}`; targeted read update if cursor advanced |
| `POST /api/color` | `{color,room?}` (palette name or `null`; stable room ID or `null` for `@global`) | `{ok:true}`; own account only; root needs no color grant, room requires `w:account.color.own` or `w:account.color.any`; metadata notice on change |
| `GET /api/bot-commands?after=...` | Session cookie | `{conversations,next}`; up to 32 entries, 512 KiB target; follow `next` with `after` |
| `GET /api/bot-commands?conversation_id=...` | Session cookie; current read/send/member access | Cached `{conversation_id,bots:[{account_id,username,commands}]}` |
| `GET /api/health` | No session required | `{ok:true}` |
| `GET /ws` | Session cookie and allowed Origin | WebSocket upgrade |

Browser API errors contain `{error:string}`. Bot API errors contain `{error:{code,message,retryable}}`; see [Bots](bots.md). Production requests must come from the configured trusted socket-peer IP. Browser and proxied requests require `X-Forwarded-Proto: https`; direct bearer-authenticated bot requests under `/api/bot/v1/` can omit it only with explicit `direct_bot_http` opt-in (default false). Login/logout/read and the WebSocket handshake check browser Origin. The reverse proxy must overwrite forwarded headers. Cookies are same-origin, HttpOnly, SameSite Strict, scoped to `base_url`, and Secure in production. No bearer token is exposed to JavaScript. Browser routes and `/ws` reject Authorization headers; bot bearer authentication is confined to `/api/bot/v1/`. Media/profile bot counterparts are listed in [Bots](bots.md#media-and-profiles-v1-extension).

Login throttling uses the client address from the configured `set_real_ip_from`
header (default `X-Forwarded-For`) only when the socket peer matches `trust`.
The proxy must overwrite it with one verified IP. Missing values use the peer
address; malformed, multi-address or repeated headers from a trusted peer reject
login with 400. Headers from other peers are ignored. Set `set_real_ip_from` to
`null` to use socket addresses only.

Login allows 10 failed attempts per IP and 20 per username in fixed 60-second
windows. Pending attempts reserve capacity; successful logins and internal
failures refund only their reservation. Successful logins do not clear earlier
failures. Tracking is bounded to 4,096 IP/username entries; saturation rejects
new entries with 429 until windows expire. Password work has its own two-worker
limit, independent of the general HTTP pool; busy password workers return 503.
Failed verification logs the client IP without credentials or request bodies.
Each failed verification also applies per-username exponential backoff of 1, 2,
4, 8, 16 and then 32 seconds, bounded by the fixed minute window. A successful
verification resets the backoff, while prior failures still count toward limits.

Authenticated HTTP requests and socket commands share an eight-job allowance
per stable account. Excess requests return 429; disconnected requests retain
their allowance until blocking work finishes. Browser and bot sockets have
separate pools of 128 and 64. Each account can hold four sockets per profile;
opening another closes its oldest socket. Bot socket controls share the
30-input/10-second per-account HTTP budget; expected discovery responses to
server requests remain exempt. Shutdown cancels sockets immediately.

Messages allow 4,000 Unicode scalar values; command lines allow 4,100, including
the command itself. Passwords allow 128 UTF-8 bytes. Reactions allow 128 Unicode
scalar values; metadata values and upload hints retain their byte limits.

## Frames

The web sends `{id:number, room:string|null, text:string, markdown?:boolean}`.
The optional `markdown` flag defaults to false and applies only to newly created
messages, including `/tell` and `/reply`. `/markdown on|off` is handled locally
in the browser. Browser aliases expand locally before sending; `/alias` is never sent to the server. Web `/user delete`, `/room delete`, and `/retract` commands wait for local confirmation. Private views send
`@direct:peer` or `@private:a:b` in `room` so command gates apply to that context. IDs correlate acknowledgements within the requesting socket. `room` is null for the command console; ordinary private sends use `/tell`. The server returns one of:

- `{kind:"notice",id,text}` or `{kind:"error",id,text}`: output for that request, visible only to its socket.
- `Snapshot`: `{kind:"snapshot",username,admin,groups,permissions,account_access,private_access,private_permissions,private_commands,policy_revision,users,online,colors,color_version,rooms,direct,private_peers,commands,available_rooms,unread}`.
- `{kind:"conversation",view,id,messages,unread,name_colors}`: the affected conversation’s latest up to 50 messages and one unread metadata object. Stable identity must match the directory; otherwise resync. Other conversations preserve their array references.
- `{kind:"bot_commands",conversation_id}`: invalidate the cached command directory; the ID is one currently readable conversation from the changed batch. Fetch all directory pages in the background.
- `{kind:"activity",conversation_id,account_id,state,expires,draft?}`: transient bot progress for current readers. `state` is working/idle; optional plain-text draft is provisional, bounded to 4,000 characters, expires after 30 seconds, and is never inserted into history.
- `{kind:"presence",online}`: authorized online account names; preserves existing message data.
- `{kind:"read",unread}`: updated unread metadata for the same username's connected devices.

The canonical frontend declarations and runtime guards are in `web/src/api/protocol.ts`; Rust serialization types are in `server/src/engine/models.rs`. Change both together. Unknown JSON is validated before entering application state. Malformed frames are ignored and malformed successful HTTP responses become explicit errors. The integration harness validates actual server frames with these same guards.

`/sudo /command [arguments]` uses the same outbound frame and acknowledgement.
It requires `/sudo` and `x:command.sudo` at global scope and elevates only the
wrapped command. Snapshots retain the caller's actual groups and permissions.
When the caller has the required contextual/global sudo grants, its `/sudo`
command descriptor includes optional `subcommands`: a list of ordinary command
descriptors available under elevation, excluding `/sudo` itself. Browser completion
uses this list for wrapped commands such as `/sudo /system configs`, without
advertising `/system` as directly available without its global command grant. Nested descriptors
do not themselves contain `subcommands`; revoking sudo removes this list.
Password commands recheck sudo authorization after asynchronous hashing.
The acknowledgement contains the wrapped command's result or error. `/sudo /room join room`
saves the caller's membership and room user grants and broadcasts updated snapshots.

Each message contains an optional boolean `markdown` (absent means plain text), `id`, `from`, nullable `to`, `text`, UTC epoch seconds in `time`, global monotonic `sequence`, `reactions` (reaction → usernames), nullable reply quote `{id,from,text,sequence}`, and mention usernames. Quotes stored before `sequence` was added report zero for it and cannot be paged to. JavaScript uses `Intl.DateTimeFormat` with the browser timezone for visible dates/times. IDs, mentions, and replies are server-generated/filtered; text remains UTF-8.

Room views contain stable `id`, `name`, `owner`, effective `permissions`, permitted
`commands`, filtered `members`, and up to 50 latest messages (1,000 tail messages globally per snapshot; some tails may be empty). Read grants govern
which views/messages/unread metadata appear; member lists have a separate grant.
`private_access` maps ordinary private views to stable IDs and exact effective
permissions/command lists. `private_permissions` and `private_commands` provide
the defaults for starting a new pair.
Su and explicitly authorized accounts see other private pairs as room-like
`@private:a:b` views, with the same history/read endpoints. `direct` contains up to 50 messages per private conversation involving the current account. `private_peers` is the persistent conversation directory, so conversations remain discoverable even when their snapshot tails are empty. Memberships/peers are loaded from server state on every login, independent of browser history.

`unread` is keyed by room name or `@direct:peer`. Each value has `count`, nullable first incoming unread sequence in `first`, latest sequence in `through`, oldest retained sequence in `oldest`, and content `revision`. Fetch the selected conversation in pages of at most 100 messages. Each room/private pair retains at most `max_messages` independently. `read` requires an existing retained sequence and only advances; own messages do not contribute to unread counts.

## History pages

`start` and `end` are inclusive **global message sequence numbers**, not array positions. Sequence gaps are normal. `before`/`after` are exclusive cursors; omit bounds for the latest page. `limit` defaults to 100 and accepts 1–100. `around` selects a page around a sequence (up to half a page before it); it cannot be combined with other bounds. `before` and `after` are mutually exclusive. Responses are always ascending.

Examples:

- `history?view=study&start=1000&end=1099`: read only the requested sequence range.
- `history?view=study&before=1000`: load the preceding page.
- `history?view=study&after=1099`: load the next page.
- `history?view=study&around=1000`: load a visible/unread anchor directly.

Use the first/last returned sequence with `has_before`/`has_after` to continue. The server never returns more than 100 records even for a wide range. Large messages can shorten a page at a 1 MiB payload target (one oversized message is still returned so paging can progress). Invalid bounds return 400; concurrent content changes return 409, prompting a fresh page request. Storage and worker failures return 503, consistently with the bot API. Access/session/resource identity are rechecked after database I/O. A selected browser window retains at most 1,000 entries, evicts the opposite edge when scrolling, and reloads evicted pages when scrolling back. Message history is not persisted in browser storage.

## Synchronization rules

The initial WebSocket snapshot and HTTP resync are baselines and do not trigger mention notifications. Subsequent conversation updates and live snapshots can notify about new mentions. HTTP refreshes run on reconnect/focus/pageshow/online and becoming visible. A generation guard prevents a delayed HTTP response from overwriting a newer socket snapshot or a signed-out session.

History requests carry an abort signal and are accepted only for the current view and stable resource ID. Each page represents a coherent content revision; live updates may already be newer. Range-preserving retries stop after three attempts and report a refresh status rather than looping indefinitely. Live tails merge into the current window by ID; cleanup/reactions/gaps refresh the visible page around its anchor, never the complete retained history. Pure appends while browsing older pages advance metadata without moving the window. Read-only updates preserve message-array references to avoid re-rendering the transcript.

Command output is bounded to the last 200 entries in tab memory, scoped to the originating view. Refresh removes it. `/clear` changes visible local history only. Ordinary sends clear the composer immediately and appear in a separate pending
list. Up to 16 message acknowledgements may overlap; commands remain exclusive.
Typing and completion stay available while requests are pending. Failed sends
retain their text and an Edit and resend action; uncertain disconnected sends
explicitly require inspecting history. No unacknowledged request is automatically
resent. Acknowledgements correlate by socket request ID and cannot clear a newer
draft or reply selection.

`online` lists active accounts with at least one authenticated WebSocket. Presence is
transient, counts multiple tabs/devices, updates on connect/disconnect, and resets
on restart. It is separate from the registered account directory in `users`.

`colors` maps account names to fixed palette names (`red` … `pink`) from each account's
`@global` `color` metadata. It covers the viewer, `users`, `private_peers`, visible
room members and names disclosed by included messages (authors, reply authors and
mentions). Each room view also carries `colors`, that room's overrides, limited to
the viewer, its visible members and names disclosed by its included messages.
Clients resolve a name as room override, then global color, then the default ink color.
Private conversations use global colors only. Older servers may omit either map.

Name color display is a browser-local opt-in, off by default. HTTP clients request
color projections with `X-Chat-Name-Colors: 1` on `/api/me` and `/api/history`;
browser sockets opt in with `/ws?colors=1`. Without this opt-in, snapshots omit
`colors`/`color_version` and room color maps, and conversation/history responses
omit `name_colors`. The flag changes projection only and grants no permissions. The browser
waits for pending commands and messages to be acknowledged before replacing its socket subscription.
Colored and uncolored baseline buffers have separate cache entries. The default
browser metadata template stores no color directory; opted-in reads build it on
demand. Toggling display reconnects that browser's socket, while HTTP reads use
the current preference.

Opted-in history pages and browser conversation frames include
`name_colors: {version,values}`. `values` resolves each disclosed author, reply author
and mention to that conversation's room override or global color; `null` explicitly
means default ink. Private pages resolve global colors only. No unrelated account or
room color entries are included. These display projections do not change the saved
room overrides shown in Settings.

Opted-in snapshots include an opaque `color_version` that changes when the active color
directory changes (including account rename, disable/delete and room color removal),
and on server restart. Clients use supplemental `name_colors` only when its `version`
matches the snapshot, refresh cached history after version changes even when the
message revision is unchanged, and resynchronize on a mismatched live frame. Cache
merges retain colors only for names still disclosed by their message window. Older
servers may omit these fields; clients then use the snapshot color maps.

`POST /api/color` authenticates the browser session, enforces the browser origin and changes only
the caller's own account. Root writes follow account ownership and need no color grant. A room
override requires `w:account.color.own` or `w:account.color.any` in that room. The room
must exist and be readable; unknown or unreadable rooms
return the same not-found error. Validation and permission checks run before charging the
per-account profile-change budget shared with avatars (4 writes per 10 seconds). No-op
requests do not consume it. The browser previews swatches locally and saves only on Apply;
Cancel, Escape and outside dismissal discard the preview.
`/user set` and `/user reset` allow your own root color, your room color with
`color.own`, or members' room colors with `color.any`. Su can manage any color.
Other metadata writes remain su-only. Both color grants are invalid outside rooms.

`/retract message-id` deletes only the requesting author’s retained message in the
selected room or their private history. It increments the conversation revision
and clears quotes of that message in retained replies. Snapshots and history
resynchronization remove the message and quotes on other devices. Ordinary admins
follow the same ownership restriction. The `su` group has any-message retraction
and read grants; stdin can locate a retained message globally by ID.

`/debug on|off` is a read-only command requiring metadata permission; a successful acknowledgement
toggles message details locally in the requesting tab. It does not change persisted
state or expose messages outside the account’s authorized conversations.
`/sudo /debug on` also enables these details locally while both global sudo grants
remain valid; revoking either grant disables the elevated toggle.

Private messages include `private_id`, the stable original participant-pair ID.
Clients route messages by this ID, so authorized inspectors can reply without
changing a private conversation’s participants.

`/man [command|topic]` returns read-only manual text as a local notice. The text
uses Markdown headings, lists, tables, fenced examples, and inline code. Every
command reply shares a lazy Markdown renderer built from validated text nodes.
HTML remains literal, images are disabled, and only safe HTTP/HTTPS/mail links
are clickable. Structured JSON, history and credential wire text is preserved;
the browser formats metadata as per-user scope sections with value/source lists and configuration,
history and newly issued credential secrets in fenced code blocks. Bot credential
listings render structured metadata as labeled fields. User getters include effective
media defaults and resolve room overrides over global values. Global custom
values require account-level read authority; room-only readers see local custom
values and effective media/cleanup limits. `@all` in the key
position requests all metadata keys; in the user position it selects all users
in the selected scope. Explicit room scope arguments require `#name`. Plain acknowledgements and errors
are escaped to preserve literal user input.
Stdin displays the same readable source. Both group
assignments and individual permission updates use `/grant` and `/revoke`.
Policy commands use `@global` for the global scope; `@server` is accepted as a
legacy alias. `/permissions`, `/grant`, and `/revoke` notices use the same
structured text format as manuals. Command metadata includes optional string
`requirements` for documentation and completion; older snapshots may omit it.
This field describes action requirements and never authorizes an operation.

`account_access` contains the signed-in account’s stable ID, effective account-scope permissions and target-authorized commands. Account settings use it for rename/password controls, including account-scoped grants.

## Bot API

The frozen `/api/bot/v1/` profile is independent of browser snapshots and command
frames. It uses stable conversation IDs, required keyed sends, desired-state
reactions, typed errors, compact directories and notification/command-discovery
sockets. Its complete contract, limits and freeze policy are in [Bots](bots.md)
and [the published schema](bot-v1.schema.json). Browser shapes above remain the
browser contract. Direct trusted bot access has the production HTTP/WS exception
specified in the bot guide.

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
New messages, replies and edits must contain content beyond whitespace, control
characters, Unicode default-ignorable characters, blank braille (U+2800) and the
object replacement placeholder (U+FFFC). The browser and server enforce this;
bot submissions use the same rule. Characters within accepted text are preserved.
Existing blank messages remain readable and are not migrated or removed.
They preserve message identity, original time, sequence, Markdown format,
reactions, and existing reply quotes, and recalculate mentions. An `edited`
boolean marks edited messages; absent values default to false. Changes persist
and are broadcast only to authorized conversation readers. Editing does not
create a new message or advance read cursors.

Browser snapshots additionally include `sync: {instance, revision}`. The instance
changes on server restart; revision monotonically combines state, presence and
read-cursor changes during that instance. Clients ignore an older full snapshot
from the same instance, preventing a queued WebSocket baseline from replacing a
newer HTTP refresh. A cached baseline retains its captured revision while its viewer-visible content is unchanged; unrelated room/session activity need not advance it. The validator accepts omitted sync metadata from older servers.
Socket open uses the server's baseline without a redundant HTTP snapshot build.

Reconnect checks `/api/session` for credential validity and obtains chat state from
the WebSocket baseline. Focus resynchronization continues to use `/api/me`.

## Responsive history and reconnects

A selected conversation paints its authorized cached window immediately while
refreshing stale revisions in the background. Known retention edges and the
current snapshot tail remove expired/retracted rows and replace edited rows in
that preview before HTTP completes. Complete snapshot tails (matching both
retained sequence edges) need no redundant history request. Cached previews are
still subject to current conversation identity and read visibility.

Up to 32 conversation scroll positions retain a stable message sequence and
pixel offset. Returning to a conversation restores that anchor, while new
conversations keep the normal unread/bottom entry behavior. Retention or resource
replacement invalidates unavailable anchors. Explicit unread jumps still work.

Paging starts in the scrolling direction using a lookahead based on observed
request latency and scroll velocity, bounded to one to three viewport heights.
Only one history request is active at a time; existing window and global cache
budgets still apply. Composer sizing coalesces draft measurements once per frame
and caches styles until width, font or root-style changes.

After a disconnect, the session check and reconnect delay run concurrently.
Current 401 responses cancel retry; stale checks cannot log out a replacement
socket. Returning online or focusing a closed connection retries immediately,
without overlapping a socket that is already connecting.

## Media uploads (draft)

All paths use `base_url`; requests use existing cookies and browser origin checks.
Allocation/completion/publication require `w:media.upload` plus read/send rights.
`conversation_id` may also be `@direct:username` to establish a first private pair.
The empty private directory and reservation commit together after validation and
quota/rate checks; rejected allocations leave no pair behind.

| Endpoint | Request | Response |
| --- | --- | --- |
| `GET /api/media/config` | Authenticated same-origin request | `{enabled,max_size}` |
| `POST /api/media/uploads` | `{conversation_id,filename,size,hints?}` | `{media_id,upload:{method:"PUT",url,headers,expires_in}}` |
| `POST /api/media/uploads/{id}/complete` | `{}` | `{media_id,path,filename,content_type,size}` |
| `POST /api/media/uploads/{id}/abandon` | `{}` | `{ok:true}`; marks an owned unpublished upload for cleanup |
| `GET /api/media/{id}/file` | Session plus current conversation read permission | 307 redirect to a 60-second signed GET; `no-store` |

Sticker libraries use separate `/api/stickers` routes; see [Stickers](#stickers).

The client sends the file directly to `upload.url` with the returned headers and
without chat cookies. The browser sets Content-Length from the selected file;
it must match the allocated size. Completion verifies storage and is idempotent;
a 100-second server budget sits below the 120-second browser timeout.
Storage failures/busy completion return 503, invalid uploads return 400. The PUT
URL admits a request for five minutes; the client transfer timeout is separately
15 minutes. The
client keeps a separate attachment preview/card and generates root-relative media
Markdown only when sending through the ordinary message path. These paths retain
the configured base path and remain usable after an origin change. A new server
instance clears unsent attachments and asks the user to attach the file again.
No upload automatically publishes a message. Managed references are derived from
canonical media paths in message text and persisted atomically; no new frame field
is required. Unbound uploads cannot be downloaded. Configuration/storage-worker
failures return 503; invalid allocations/completions return 400; unavailable media
returns 404; invalid sessions return 401; untrusted origins return 403.
See [Media uploads](media-uploads.md) for limits and privacy behavior.

Storage outages do not hide configured upload controls. Allocation authenticates
and validates the request, then performs a signed object HEAD with a three-second
timeout before reserving pending quota or issuing a URL. Connection failures and
storage server errors return HTTP 503: “Upload rejected: media storage is
unavailable. Try again later.” Authorization is rechecked after that I/O. The
probe needs no listing/write permission; missing-object 403/404 responses prove
reachability, while S3 still enforces actual PUT permissions and size constraints.

Media attachment metadata: authenticated `GET /api/media/{id}/info` returns the
upload-completion fields `{media_id,path,filename,content_type,size}` plus
`expires` (Unix seconds, or null for permanent files) and `expired` (boolean).
Only retained, published attachments readable by the current viewer are visible
(otherwise 404). Normal info and file reads reject expiry with 404, preserving
access enforcement for existing clients and media workers. The UI explicitly requests
`/info?include_expired=true` to retrieve metadata, including the original filename,
for expired attachments even after object cleanup; it grants no access to bytes.
No storage request is made. `GET /api/media/{id}/file?download=true` forces
attachment disposition; the default permits native recognized media playback.
Markdown link titles `media:audio`, `media:video`, `media:html`, and `media:svg`
are optional UI hints; the client verifies MIME with the info endpoint on click.

Browser media bytes are explicitly persisted by the application-scoped media worker
under canonical file URLs (playback/download queries share content). Each cached
read still calls the authenticated `/info` endpoint; no S3 GET is needed on a hit.
Only full 200 responses are cached; hits refresh eviction recency, and ranges can
be served from a complete cached object. Every range still checks `/info` so
access revocations take effect immediately. Expired metadata stops cache reuse
with 410 and never fetches storage. Network redirects and storage remain `no-store`. See [Media uploads](media-uploads.md)
for bounds, full-object caching and fallback behavior.

## Object commands and upload hints

The command catalogue advertises `/user` and `/room` families. See [Commands](commands.md)
for scoped listing, metadata and lifecycle syntax. `/user get` returns a local JSON
notice shaped as `{username: {scope: {key: {value, source}}}}`.
The `@global` section appears only with account/global read authority; a room read
includes its `#name` section. Room-only readers receive effective room limits, but
no global section or account quota. Sources are `config`, `role`, `@global`, or `#name`. Account-wide quota appears
only under `@global`. A key filter applies to both sections. Other metadata is absent
from broadcast snapshots. The public avatar profile key is projected separately as `avatars`. Bulk writes
and grant changes return one acknowledgement only after an atomic commit.

Upload hints are an optional object with keys `filename`, `content-type`, `label`,
and `client`, with 1–256 printable ASCII bytes per value and no surrounding spaces.
Allocation rejects unknown/invalid hints before storage I/O. Accepted fields become
signed `x-amz-meta-client-*` headers in the upload ticket. Clients send every returned
header and never attach chat credentials. Final CopyObject replaces metadata with
these validated hints. All hints remain untrusted and have no authorization or
MIME/size/retention effects. Unicode hint values must be percent encoded.


`/clean age scope [--dry-run]` requires an explicit room/private/global scope.
Preview runs under the read-only command path, with identical authorization and
metadata cleanup limits but no audit, revision or database writes. Execution reports
Markdown counts of deleted messages and removed attachment references; S3 cleanup
and quota release follow confirmed asynchronous storage deletion.
All `/user set` writes (including bulk, removals and media metadata) require su.
Admins retain metadata inspection through `r:account.metadata`; write flags and
room policy management do not override the su-only mutation rule.

`history.clean_min_age` appears in metadata sections with its source and effective
value. Values `0` and `-1` mean disabled and unrestricted; positive values are
minimum message ages in seconds. Browser output formats positive ages as durations
and labels room values as `set here` or `inherited`; global values have no hints.
Source detail stays in the wire result. Cleanup accepts `-1` as an explicit all-history
request. Su/authorized sudo bypass command/action and cleanup-limit checks;
ordinary execution and previews use the same per-target metadata resolution.

`/user reset who key|@all [scope]` removes explicit metadata overrides in one
selected scope, atomically across bulk targets. It does not change credentials.
`/user reset who password new-password` is the explicit password-reset form,
using the existing asynchronous hashing, authority recheck and session revocation.

## User avatars

Snapshots include `avatars`, a username-to-image-URL map for visible identities.
Older servers may omit it. Only the global `avatar` metadata key is projected;
other metadata remains private. External URLs require HTTPS, no credentials or
controls, and at most 2048 bytes. Clients load external URLs only after the viewer
enables Load external avatars in Security (browser-local, default off), with no
referrer. External bytes and pixels are not bounded by the server; opting in
permits requests to external hosts. Missing/failed images reserve no layout space;
a previously loaded frame stays visible while a replacement loads.

All avatar writes authenticate the current session and enforce browser origin
checks; they only update the caller's profile, independently of room upload grants.
General metadata commands retain their su requirement.

| Endpoint | Request | Response |
| --- | --- | --- |
| `POST /api/avatars` | `{url:"https://…"}` or `{url:null}` | `{ok:true}`; set or remove own avatar |
| `POST /api/avatars/uploads` | `{size,upload_id?}` (1 byte–64 KiB, optional canonical UUID) | Existing media upload ticket shape; same owned pending ID/size replays without another allocation |
| `POST /api/avatars/uploads/{id}/complete` | `{}` | `{ok:true,media_id,path}`; verify bytes and set own avatar |
| `GET /api/avatars/{id}/info` | Authenticated same-origin read | `{size,content_type,filename}` after current profile/visibility checks; `no-store` |
| `GET /api/avatars/{id}/file` | Authenticated same-origin read | 307 to private, 60-second signed S3 GET; `no-store` |

Upload PUTs target staging `uploads/<id>` with the returned headers and no chat
cookies. Completion copies verified PNG/JPEG/WebP bytes to immutable
`avatars/<stable-account-id>/<id>` before updating metadata and broadcasting a
snapshot. The stored link `/api/avatars/<id>/file` is deployment-relative: clients
resolve it under their application base path. Active avatars have no retention
expiry. Replaced/removed avatars stop receiving new signed reads immediately;
already issued URLs expire within 60 seconds. Unreferenced objects use the existing
cleanup worker, staging grace, retries, quota and allocation rate limits.
No image bytes are stored in SQLite or streamed through chat. Reads require a
current active owner/profile reference and viewer visibility through the directory,
a readable room's members or retained authors, or readable private peers/members/authors.
The media worker privately caches immutable avatar bytes and rechecks `/info` before
every reuse; removal, disablement, logout and lost visibility deny cached reads.
The browser supplies a generated `upload_id`, replays an uncertain allocation with
that same ID, and abandons the known reservation on cancellation/failure. A different
owner/size, completed, expired or abandoned allocation cannot reuse the ID.
Avatar allocation skips the preliminary storage availability probe; PUT/completion
report storage failures. Completion retains its immutable-copy validation and
post-I/O authorization checks.

## Stickers

Each account has a private, permanent sticker library. Library reads, writes and
downloads are owner-only and do not need room permissions; sending needs
`w:media.upload` in the target conversation.

| Endpoint | Request | Response |
| --- | --- | --- |
| `GET /api/stickers` | Authenticated same-origin read | `{enabled,max_size,limit,stickers:[{id,path,content_type,size}]}`, newest first |
| `POST /api/stickers/uploads` | `{size}` (1 byte–`max_size`) | Existing media upload ticket shape; rejected at the library `limit` |
| `POST /api/stickers/uploads/{id}/complete` | `{}` | One sticker entry; bytes must be PNG/JPEG/GIF/WebP |
| `GET /api/stickers/{id}/file` | Owner session | 307 to private, 60-second signed S3 GET; `no-store` |
| `POST /api/stickers/{id}/send` | `{conversation_id}` | Completed media shape for a conversation copy |
| `POST /api/stickers/{id}/delete` | `{}` | `{ok:true}`; already sent copies remain |
| `POST /api/stickers/collect` | `{media_id}` | One new sticker entry copied from a readable sent sticker |

`send` copies the sticker into an ordinary conversation upload that follows the
room's retention, quota and cleanup rules. A live, referenced or recently pending
copy of the same sticker in the same conversation by the same sender is reused.
Clients send it as `![sticker](/api/media/<id>/file "media:sticker")`; the title
is a display hint only. `collect` accepts only media that was sent from a
sticker. Browsers resize PNG/JPEG/WebP to at most 512 px and encode WebP; GIFs
are uploaded unchanged. Bots have no sticker endpoints.

External avatar URLs require an exact host in server `avatar_hosts` (default empty).
Only HTTPS without userinfo is accepted; subdomains and suffix lookalikes do not
inherit allowance. Disallowed stored URLs are omitted from snapshots. Managed
avatar uploads remain available independently and retain the 64 KiB and image
type limits. The browser’s external-avatar preference still defaults off.

Browser engine failures include `error`, `code` and `retryable` fields. Typed domain, snapshot, media,
avatar, session and history operations distinguish `invalid_request` (400),
`not_found` (404), `forbidden` (403), `conflict` (409), `unavailable` (503) and
`internal` (500). Conflict and unavailable responses allow retry. Private object
lookups conceal access failures as not found while preserving unavailable errors.
Production deployments at `/` use `__Host-chat_session`; other deployments use
`chat_session`. Requests with duplicate selected session cookies do not authenticate.

Native v1 bot commands preserve domain failure classes: invalid arguments return
400, denied authority returns 403, conflicting state returns 409, and persistence
failures return retryable 503. Database diagnostics remain server-side. The v1
contract maps internal backend failures to its existing `unavailable` code.
Browser command frames and console output retain their existing text presentation;
error types are preserved inside the engine until those final boundaries.

Avatar mutations emit a browser-only `{ "kind": "avatar", "username": "alice",
"url": "https://example.com/avatar.png" }` profile update. `url: null` removes that
profile's avatar. Apply only that entry; preserve the loaded conversation windows.
Recipients must currently be able to see the account through the directory, room/
private membership or an authored message in readable retained history. Missing or
inaccessible profiles emit no frame. Bot sockets skip these events.
Self-service avatar changes and managed-avatar publication share a four-attempt
budget per stable account per ten seconds; unchanged values and repeated completion
of the current avatar do not spend it. Exhaustion returns retryable 429. Other
accounts keep independent capacity. Upload allocation retains its existing quotas.


Metadata changes send a baseline to affected viewers, including newly authorized and
revoked viewers. Unrelated clients may retain an older `policy_revision`; it describes
their last captured snapshot and is not a global notification sequence. Permission
checks always use current server authority. Bot credential changes can also emit
compact `bot_commands` invalidations for readable conversations without a baseline.


Browser plugin command discovery uses `GET /api/bot-commands` with session auth.
It returns `{conversations:[{conversation_id,bots}]}` for accessible conversations,
with command metadata filtered by current viewer and bot authority. One directory
request is cached locally and rechecked every 120 seconds; server invalidations
refresh it earlier. Bot v1 `commands_response` requires `expires_in` (30–3,600
seconds). The server owns expiry refresh and bounds the cache; see [Bots](bots.md#command-discovery).

Browser snapshots may include `bot_credentials:[{id,bot,label,kind}]` for `/bot`
credential completion. The list contains only credentials the viewer may manage
under global `/bot`, account credential and protected-su checks. `kind` is
`issuer` or `worker`; secrets and digests are omitted. Credential metadata changes
refresh authorized managers in addition to bot/owner audiences. Omitted fields
remain compatible with older servers.
