# HTTP and WebSocket protocol

All paths are relative to the configured `base_url`. Nginx serves web assets and proxies HTTP API requests and WebSockets to the server's single backend listener. At root hosting the endpoints are:

| Endpoint | Request | Response |
| --- | --- | --- |
| `POST /api/login` | `{username,password}` | `{ok:true}` and HttpOnly session cookie |
| `POST /api/logout` | `{}` | `{ok:true}` and cleared cookie |
| `GET /api/me` | Session cookie | Authorized `Snapshot` |
| `GET /api/history?view=...` | Room name or `@direct:peer` | `{view,messages,revision}` |
| `POST /api/read` | `{view,through}` | `{ok:true}`; targeted read update if cursor advanced |
| `GET /api/health` | No session required | `{ok:true}` |
| `GET /ws` | Session cookie and allowed Origin | WebSocket upgrade |

Non-success HTTP responses contain `{error:string}`. Production requests must come from the configured trusted socket-peer IP and include `X-Forwarded-Proto: https`. Login/logout/read and the WebSocket handshake check browser Origin. The reverse proxy must overwrite forwarded headers. Cookies are same-origin, HttpOnly, SameSite Strict, scoped to `base_url`, and Secure in production. No bearer token is exposed to JavaScript.

Login throttling uses the client address from the configured `set_real_ip_from`
header (default `X-Forwarded-For`) only when the socket peer matches `trust`.
The proxy must overwrite it with one verified IP; invalid or absent values use
the peer address. The allowance is 10 login attempts per IP per minute.

## Frames

The web sends `{id:number, room:string|null, text:string}`. IDs correlate acknowledgements within the requesting socket. `room` is null for the command console and private views; private sends use `/tell`. The server returns one of:

- `{kind:"notice",id,text}` or `{kind:"error",id,text}`: output for that request, visible only to its socket.
- `Snapshot`: `{kind:"snapshot",username,admin,users,online,rooms,direct,private_peers,commands,available_rooms,unread}`.
- `{kind:"read",unread}`: updated unread metadata for the same username's connected devices.

The canonical frontend declarations and runtime guards are in `web/src/api/protocol.ts`; Rust serialization types are in `server/src/engine/models.rs`. Change both together. Unknown JSON is validated before entering application state. Malformed frames are ignored and malformed successful HTTP responses become explicit errors. The integration harness validates actual server frames with these same guards.

Each message contains `id`, `from`, nullable `to`, `text`, UTC epoch seconds in `time`, global monotonic `sequence`, `reactions` (reaction → usernames), nullable reply quote `{id,from,text}`, and mention usernames. JavaScript uses `Intl.DateTimeFormat` with the browser timezone for visible dates/times. IDs, mentions, and replies are server-generated/filtered; text remains UTF-8.

Room views contain `name`, `members`, and the latest up to 50 messages. `direct` contains up to 50 messages per private conversation involving the current account. `private_peers` is the persistent conversation directory, so conversations remain discoverable even when their snapshot tails are empty. Memberships/peers are loaded from server state on every login, independent of browser history.

`unread` is keyed by room name or `@direct:peer`. Each value has `count`, nullable first incoming unread sequence in `first`, latest sequence in `through`, oldest retained sequence in `oldest`, and content `revision`. Fetch the selected conversation's full retained history on demand. Each room/private pair retains at most `max_messages` independently. `read` requires an existing retained sequence and only advances; own messages do not contribute to unread counts.

## Synchronization rules

The initial WebSocket snapshot and HTTP resync are baselines and do not trigger mention notifications. Subsequent live snapshots can notify about new mentions. HTTP refreshes run on reconnect/focus/pageshow/online and becoming visible. A generation guard prevents a delayed HTTP response from overwriting a newer socket snapshot or a signed-out session.

History requests carry an abort signal and are accepted only for the current view and matching revision. Live snapshot tails replace cached messages by ID; cleanup, reactions, or gaps cause a fresh retained-history fetch. Read-only updates preserve message-array references to avoid re-rendering the transcript.

Command output is bounded to the last 200 entries in tab memory, scoped to the originating view. Refresh removes it. `/clear` changes visible local history only. Unacknowledged requests are never resent automatically: after a disconnect, preserve the draft and ask the user to inspect history before resending.

`online` lists active accounts with at least one authenticated WebSocket. Presence is
transient, counts multiple tabs/devices, updates on connect/disconnect, and resets
on restart. It is separate from the registered account directory in `users`.

`/retract message-id` deletes only the requesting author’s retained message in the
selected room or their private history. It increments the conversation revision
and clears quotes of that message in retained replies. Snapshots and history
resynchronization remove the message and quotes on other devices. Web admins
follow the same ownership restriction. Stdin `su` can retract any retained room or
private message by ID without selecting a conversation.

`/debug on|off` is a read-only, web-admin command; a successful acknowledgement
toggles message details locally in the requesting tab. It does not change persisted
state or expose messages outside the admin’s authorized conversations.
