# HTTP and WebSocket protocol

All paths are relative to the configured `base_url`. Nginx serves web assets and proxies HTTP API requests and WebSockets to the server's single backend listener. At root hosting the endpoints are:

| Endpoint | Request | Response |
| --- | --- | --- |
| `POST /api/login` | `{username,password}` | `{ok:true}` and HttpOnly session cookie |
| `POST /api/logout` | `{}` | `{ok:true}` and cleared cookie |
| `GET /api/me` | Session cookie | Authorized `Snapshot` |
| `GET /api/history?view=...` | View + sequence cursors/range + limit | `{view,messages,revision,has_before,has_after}` |
| `POST /api/read` | `{view,through}` | `{ok:true}`; targeted read update if cursor advanced |
| `GET /api/health` | No session required | `{ok:true}` |
| `GET /ws` | Session cookie and allowed Origin | WebSocket upgrade |

Browser API errors contain `{error:string}`. Bot API errors contain `{error:{code,message,retryable}}`; see [Bots](bots.md). Production requests must come from the configured trusted socket-peer IP. Browser and proxied requests require `X-Forwarded-Proto: https`; direct bearer-authenticated bot requests under `/api/bot/v1/` can omit it. Login/logout/read and the WebSocket handshake check browser Origin. The reverse proxy must overwrite forwarded headers. Cookies are same-origin, HttpOnly, SameSite Strict, scoped to `base_url`, and Secure in production. No bearer token is exposed to JavaScript.

Login throttling uses the client address from the configured `set_real_ip_from`
header (default `X-Forwarded-For`) only when the socket peer matches `trust`.
The proxy must overwrite it with one verified IP; invalid or absent values use
the peer address. The allowance is 10 login attempts per IP per minute.

## Frames

The web sends `{id:number, room:string|null, text:string, markdown?:boolean}`.
The optional `markdown` flag defaults to false and applies only to newly created
messages, including `/tell` and `/reply`. `/markdown on|off` is handled locally
in the browser. Browser aliases expand locally before sending; `/alias` is never sent to the server. Web `/rm` and `/retract` commands wait for local confirmation. Private views send
`@direct:peer` or `@private:a:b` in `room` so command gates apply to that context. IDs correlate acknowledgements within the requesting socket. `room` is null for the command console; ordinary private sends use `/tell`. The server returns one of:

- `{kind:"notice",id,text}` or `{kind:"error",id,text}`: output for that request, visible only to its socket.
- `Snapshot`: `{kind:"snapshot",username,admin,groups,permissions,account_access,private_access,private_permissions,private_commands,policy_revision,users,online,rooms,direct,private_peers,commands,available_rooms,unread}`.
- `{kind:"conversation",view,id,messages,unread}`: the affected conversation’s latest up to 50 messages and one unread metadata object. Stable identity must match the directory; otherwise resync. Other conversations preserve their array references.
- `{kind:"read",unread}`: updated unread metadata for the same username's connected devices.

The canonical frontend declarations and runtime guards are in `web/src/api/protocol.ts`; Rust serialization types are in `server/src/engine/models.rs`. Change both together. Unknown JSON is validated before entering application state. Malformed frames are ignored and malformed successful HTTP responses become explicit errors. The integration harness validates actual server frames with these same guards.

`/sudo /command [arguments]` uses the same outbound frame and acknowledgement.
It requires `/sudo` and `x:command.sudo` at global scope and elevates only the
wrapped command. Snapshots retain the caller's actual groups and permissions.
Password commands recheck sudo authorization after asynchronous hashing.
The acknowledgement contains the wrapped command's result or error. `/sudo /join room`
saves the caller's membership and room user grants and broadcasts updated snapshots.

Each message contains an optional boolean `markdown` (absent means plain text), `id`, `from`, nullable `to`, `text`, UTC epoch seconds in `time`, global monotonic `sequence`, `reactions` (reaction → usernames), nullable reply quote `{id,from,text}`, and mention usernames. JavaScript uses `Intl.DateTimeFormat` with the browser timezone for visible dates/times. IDs, mentions, and replies are server-generated/filtered; text remains UTF-8.

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

History requests carry an abort signal and are accepted only for the current view, stable resource ID and matching revision. Live tails merge into the current window by ID; cleanup/reactions/gaps refresh the visible page around its anchor, never the complete retained history. Pure appends while browsing older pages advance metadata without moving the window. Read-only updates preserve message-array references to avoid re-rendering the transcript.

Command output is bounded to the last 200 entries in tab memory, scoped to the originating view. Refresh removes it. `/clear` changes visible local history only. Unacknowledged requests are never resent automatically: after a disconnect, preserve the draft and ask the user to inspect history before resending.

`online` lists active accounts with at least one authenticated WebSocket. Presence is
transient, counts multiple tabs/devices, updates on connect/disconnect, and resets
on restart. It is separate from the registered account directory in `users`.

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
uses headings, bullet lists, fenced examples, and inline code. The web renders
these as structured documentation using text nodes; HTML and links are not interpreted.
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
reactions, typed errors, compact directories and notification/configuration
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
They preserve message identity, original time, sequence, Markdown format,
reactions, and existing reply quotes, and recalculate mentions. An `edited`
boolean marks edited messages; absent values default to false. Changes persist
and are broadcast only to authorized conversation readers. Editing does not
create a new message or advance read cursors.
