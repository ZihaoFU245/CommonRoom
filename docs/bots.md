# Bot API v1

V1 is the chat transport for external agents under `/api/bot/v1/`.
The [JSON Schema](bot-v1.schema.json) and live contract tests define the API.
Workers must use the documented credential kinds and checkpoint format.
There are no unversioned bot routes. Native server command execution is provided
through the authenticated v1 `/commands` endpoint.

## Identity and provisioning

A bot is an ordinary account with a machine registration. The server owns
identity, grants and chat history; an external worker owns models, tools and
configuration. This keeps the API independent of any agent framework.

Global administrators, su or stdin manage registrations and issuer tokens:

```text
/bot new helper #support
/bot token new helper issuer
/bot info helper #support
/bot token ls helper
/bot token rm helper CREDENTIAL_ID
/bot rm helper @global
```

Creation requires a new name and the creator's account-create, room-invite and
corresponding content/command grants. A selected room supplies the default scope;
Console/stdin default to `@global`; private contexts require an explicit room or `@global`. Use `@global` explicitly from a room
to grant broad room/private content access. Room admins alone cannot provision
or issue credentials. Su accounts remain protected from ordinary admins.

Room defaults grant discover/join/read/member-list/send/react and
`/help`, `/man`, `/whoami`, `/user ls`, `/history`, `/room join`, `/reply`, `/react`.
There is no group assignment, password, private-create or management permission.
These command grants support existing engine authorization; v1 exposes only the
HTTP operations below. `/grant` and `/revoke` adjust access. `/room kick` removes room
participation; `/user disable` suspends tokens; rename preserves stable IDs; account
deletion removes tokens and retry receipts. Name reuse restores no old authority.

Tokens are shown once, hashed at rest and individually revocable, with at most
eight unique labels per account across both credential kinds. `/bot token new`
creates an **issuer** token. It can only call `POST /api/bot/v1/credentials`;
it cannot read chat, change profiles, upload media or open a socket. Keep it
with a trusted plugin supervisor. The supervisor requests a **worker** token:

```http
POST /api/bot/v1/credentials
Authorization: Bearer bot_<issuer-secret>
Content-Type: application/json

{
  "label": "worker-session-123",
  "expires": 1793206800,
  "conversations": ["CONVERSATION_ID"],
  "operations": ["read", "send", "react", "activity"]
}
```

`expires` is a required future Unix timestamp in seconds, at most 24 hours ahead;
replace the example timestamp before use. A successful HTTP 200 response is:

```json
{"id":"CREDENTIAL_ID","token":"bot_<worker-secret>","expires":1793206800}
```

Use the returned secret as the Bearer credential for worker requests. It is
returned once. Worker tokens cannot issue tokens, including account-wide workers.
All request fields are required. `conversations` is an array of stable IDs or
explicit `null` to delegate the account's live conversation scope. An empty array
grants no chat access. Operations are `read`, `send`, `react`, `read_ack`,
`activity`, `config`, `commands`, `media_upload`, `profile`, and `create`.
`commands` and `create` require `conversations:null`. Uploads require all of
`read`, `send` and `media_upload`.

Issuance checks current account permissions: every explicitly listed conversation
must support every requested operation. For `null`, each operation must be possible
under the account's current grants somewhere; it does not grant access everywhere.
During use, the worker's operations and scope also intersect live account grants.
Permission revocation therefore applies before token expiry. Own appearance is
self-service through explicit `profile` delegation.

`/me` exposes the worker's credential kind, ID, expiry, scope and operations.
Issuer and worker tokens cannot authenticate browser transports. Revocation and
expiry are checked on requests and sockets; a heartbeat closes an idle expired
socket within 25 seconds. Remove expired credentials with `/bot token rm` before
reusing their label or capacity. `/bot token ls` returns metadata, never secrets.
Browser output formats creation and expiry timestamps in local time; the JSON
metadata retains epoch seconds. `/bot token rm ID` locates the authorized bot
automatically, while `/bot token rm BOT ID` remains supported. Tab completion
shows bot names, labels and kinds, then inserts the credential ID. These hints
are filtered by the same account/credential grants and protected-su checks as
revocation and refresh after issuance, rename and revocation.
Deleting the bot removes all issuer and worker credentials and retry receipts.
Credentials persist across restart. SQLite v19 upgrades to v20 atomically to add
explicit credential kinds; PostgreSQL retains its current unpublished v1 marker.
Never log credentials or message bodies.

## HTTP contract

All routes are relative to the configured `base_url`. Each requires
`Authorization: Bearer bot_...`; cookies alone do not authenticate bots.
Responses include `Bot-Api-Version: 1`. Use HTTPS/WSS through the configured
proxy. Production always requires the trusted socket-peer IP. Direct HTTP/WS
without a forwarded protocol header requires explicit `direct_bot_http` opt-in
(default false) and a private/loopback trusted peer. Forwarded requests must
indicate HTTPS, including when direct mode is enabled. A browser Origin is
unnecessary. See [Deployment](deployment.md#explicit-direct-bot-http).

| Method and endpoint                                                | Payload / result                                                                                                      |
| ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| `GET /api/bot/v1/me`                                               | `Snapshot`: account, credential context and readable conversation directory                                           |
| `POST /api/bot/v1/credentials`                                     | `{label,expires,conversations,operations}`; `{id,token,expires}` (secret shown once)                                  |
| `POST /api/bot/v1/conversations`                                   | `{kind:"room",name}` or `{kind:"direct",username}`; `{conversation_id}`                                               |
| `GET /api/bot/v1/commands?conversation_id=ID`                      | Current native server command catalogue under the bot account’s grants                                                |
| `POST /api/bot/v1/commands`                                        | `{conversation_id,command}` plus required `Idempotency-Key`; `{result,replayed,expires}`; executes as the bot account |
| `GET /api/bot/v1/commands/receipts/KEY`                            | `{state:"not_found"}` or saved `{state:"succeeded"                                                                    | "failed",result,error,created,expires}` |
| `GET /api/bot/v1/message-context?conversation_id=ID&message_id=ID` | Verified source author, roles, inspectable permissions and bot ownership facts                                        |
| `GET /api/bot/v1/history`                                          | `conversation_id`, sequence bounds and `limit` query; `HistoryResponse`                                               |
| `POST /api/bot/v1/messages`                                        | `MessageRequest` plus required `Idempotency-Key`; `SendResponse`                                                      |
| `POST /api/bot/v1/read`                                            | `{conversation_id,through,acknowledge_shared_unread:true}`; `{ok:true}`                                               |
| `PUT /api/bot/v1/reactions`                                        | `{conversation_id,message_id,reaction,active}`; `{ok:true}`                                                           |
| `GET /api/bot/v1/ws`                                               | Authenticated notification/command-discovery socket                                                                       |
| `GET /api/bot/v1/schema`                                           | The published JSON Schema                                                                                             |

The [schema](bot-v1.schema.json) is normative for shapes and types. Validate the
named `definitions` entry for each payload, or `ServerFrame` / `ClientFrame` for
socket directions. Unknown request fields are rejected. UTF-8 byte bounds,
permissions and history bound combinations have additional server checks.
HTTP has no request-correlation `id`; its response belongs to the request.

A snapshot contains `{kind:"snapshot",account:{id,username},credential,conversations}`.
Each conversation has stable `id`, current display/routing label `view`,
`permissions:{send,reply,react}`, filtered `members`, `content_revision`, and `unread` metadata. Unavailable unread
metadata is `null`; healthy conversations still appear. Workers must defer that
conversation's baseline until metadata recovers, rather than treating it as zero.
The directory contains only readable conversations. An empty member list can
mean member-list access is absent. The optional `policy_revision` is the current authorization policy revision;
workers may share native command catalogue reads until it or the credential
changes. `server_instance` changes on server restart and also invalidates that
cache. Snapshots without `policy_revision` require live catalogue reads.
The snapshot carries no message tails,
command catalogue or browser settings.

Use the **conversation ID** in every operation. IDs are opaque strings of 1–128 ASCII letters, digits, dots, underscores, colons or hyphens;
never derive them from names. Rename preserves an ID. Delete/recreate uses a
new ID, so an old request cannot be redirected into a replacement conversation.
Existing private conversations work the same way; their participants remain
fixed. `POST /conversations` requires an account-wide worker with the `create` operation and existing
account permissions. Rooms require `/room` and `w:room.create`; direct conversations
require `/tell`, private send access, and `w:private.create` when the pair does not
already exist. Creation does not grant authority that the account lacks. Existing
direct pairs return the same ID without sending a message; room name collisions
return `conflict`. Bots created with room defaults need explicit operator grants
before initiating either kind. Creation is not automatically retried by the client.

## Media and profiles (v1 extension)

Bot bearer credentials are accepted only under `/api/bot/v1/`. Browser routes,
including `/api/me`, `/api/media/*`, `/api/avatars/*`, `/api/color` and `/ws`,
reject Authorization headers even when a valid session cookie accompanies them.
New browser routes do not inherit machine authentication. The bot adapters share
media/profile services and engine authorization with browser handlers, rather
than reusing browser authentication.

These routes accept expiring worker tokens with scoped or account-wide access.
Issuer tokens cannot call them.
Worker capabilities intersect their explicit operations, conversation IDs and the
account's current grants. Existing worker tokens gain no additional operations
automatically: issue a replacement with the required capabilities. All routes
require bearer authentication, accept no session-only authentication, and require
no Origin header. Existing production ingress rules apply.

| Capability | Worker-token requirements |
| --- | --- |
| Media configuration | `read` (no conversation-specific data) |
| Media info/download | `read` and the attachment's conversation ID |
| Media allocation/completion/abandon | `media_upload`, `read`, `send` and the upload's conversation ID |
| Avatar info/download | `read`; another account must be visible through an allowed conversation's members/peer or retained history |
| Own avatar set/upload/complete | `profile` |
| Own global name color | `profile` |
| Own room name color | `profile` and that room's conversation ID |

`profile` explicitly delegates the bot account's own global appearance, even on a
conversation-scoped token. It does not authorize another account's profile or
arbitrary metadata. Omit it from room workers that should not change shared
appearance. A `read` token may inspect its own selected avatar; global account
directory grants and visibility through other conversations cannot expose other
accounts' avatars outside its scope. Account-wide tokens have no additional token
scope restrictions, but still need the same account grants and ownership checks.

| Method and endpoint (under `/api/bot/v1`) | Request / response schema |
| --- | --- |
| `GET /media/config` | `MediaConfig` |
| `POST /media/uploads` | `MediaUploadRequest` → `UploadResponse` |
| `POST /media/uploads/{id}/complete` | No request body required; `MediaResponse` |
| `POST /media/uploads/{id}/abandon` | No request body required; `Ok` |
| `GET /media/{id}/info` | Optional `MediaInfoQuery`; `MediaInfo` |
| `GET /media/{id}/file` | Optional `MediaDownloadQuery`; 307 redirect |
| `POST /avatars` | `AvatarRequest` → `Ok` |
| `POST /avatars/uploads` | `AvatarUploadRequest` → `UploadResponse` |
| `POST /avatars/uploads/{id}/complete` | No request body required; `AvatarCompleteResponse` |
| `GET /avatars/{id}/info` | `AvatarInfo` |
| `GET /avatars/{id}/file` | 307 redirect |
| `POST /color` | `ColorRequest` → `Ok` |

For attachments, allocate with `{conversation_id,filename,size,hints?}`, PUT
bytes directly to the returned `upload.url` using exactly its supplied headers,
then complete the upload. Allocation/completion still require current
conversation read/send and `w:media.upload`; default bot provisioning does not
grant upload. Quotas, size limits, ownership, retention and cleanup are shared
with browser uploads. Abandon removes an unused reservation. Download and info
require current conversation read access and an attached, available object;
`include_expired=true` on info can inspect expired attachment metadata.
`download=true` requests attachment disposition on file downloads.

The returned `path` is the canonical browser media path used in message Markdown,
e.g. `/api/media/ID/file` (including the configured base path). Send that path in
`POST /messages`; do not replace stored Markdown links with bot-specific URLs.
For machine retrieval, request `/api/bot/v1/media/ID/info` or `/file` instead.
Downloads return a presigned bucket URL in Location, valid for at most 60 seconds
and shortened to remaining retention. Follow it without forwarding the bot bearer
header to storage. Retraction/expiry/access revocation prevents issuance of new
URLs; an already-issued URL can remain usable until it expires.

Avatar allocation uses `{size,upload_id?}` (1–65,536 bytes); the optional canonical
UUID supports allocation retries. PUT and complete as above. Completion validates
PNG/JPEG/WebP and selects the avatar atomically. Avatar `path` values retain the
canonical `/api/avatars/ID/file` form; machine downloads use the v1 avatar route.
Avatar uploads are self-service and do not require `w:media.upload`, but retain
account quota and allocation limits. `POST /avatars` sets an allowed external
HTTPS URL with `{url}` or clears it with `{url:null}`; `avatar_hosts` still applies.

`POST /color` accepts `{color,room?}`. Color is one of the fixed palette names or
null to clear; room is a stable room ID, omitted/null for the global default.
Global own-color updates need no color grant; room updates require the existing
own-color permission. These routes cannot edit another account's profile or
arbitrary metadata. Use authorized native `/user get` and `/user set` commands for
metadata. The v1 snapshot/message projections are unchanged by this extension.

Uploads and profile changes use their existing allocation/profile rate limits;
they do not acquire message-send idempotency receipts. Retry avatar allocation
with the same upload_id; repeating completion cannot select an older avatar over
a newer one. Attachment allocation has no idempotency key: a lost allocation
response may leave a pending reservation for cleanup. This remains API v1; no v2
namespace or automatic browser-route fallback exists.

## Messages and reactions

Send `{conversation_id,text,reply_to?,markdown?}` with a fresh idempotency key.
Text is trimmed, supports 1–4,000 Unicode characters, and remains literal even
when it starts with `/`. Markdown defaults to true; send `markdown:false` for plain text. A reply needs current read,
send and `/reply` grants; all v1 sends need read/send access to return the result.
A reply target must be retained in the selected conversation.

Both plain sends and replies return `{message,replayed}`. A message contains
stable `id` and `author_id`, current author label `from`, text/format/edit marker,
UTC epoch-second `time`, global `sequence`, nullable reply quote, filtered
mention usernames and reactions. Reply quotes capture at most 160 characters.
For output over 4,000 characters, the reference client's `sendLong` splits at
Unicode character boundaries into independently keyed messages prefixed `[i/n]`.
Each part gets a stable numbered output slot. A failed batch may leave a visible
partial response; replay resumes its saved parts. There is no batch transaction.

There is no automatic invocation: an agent chooses how to act on `mentions`.
Each mention starts one worker turn; the agent may use tools, reply once, or
finish silently. The message’s `from` field supplies the sender’s username alongside
`author_id`; workers must include both when constructing the current model input.

Reaction updates set this account's membership to `active:true` or `false`.
Repeating the same request is a no-op, unlike a toggle command. Reactions support
1–128 Unicode characters without controls or leading/trailing whitespace, with
at most 32 distinct reactions per message. Read/react and `/react` grants apply.
An HTTP success does not promise exactly-once external tool execution.

## Retries and history

`Idempotency-Key` is required on sends: one header containing 1–128 ASCII letters,
digits, dots, underscores, colons or hyphens. Persist the key and payload before
sending. Keys belong to the stable account across credential rotation and rename.
Their fingerprint covers conversation ID, trimmed text, reply target and format.

A matching retry returns the current retained message with `replayed:true`,
without another send or broadcast. Different content returns
`idempotency_conflict`. A receipt and message commit in one SQLite transaction
and survive server restart. Removed messages return `message_gone`; missing
conversations return `not_found`. Current grants and credentials are rechecked.

Receipts last 24 hours from the original send; replay does not extend expiry.
Each account holds at most 4,096 live receipts. Capacity rejects new keys rather
than evicting a guarantee. After expiry, a reused key is a new send. Receipts
store a digest and identifiers, not copied message text. Account deletion clears
them. These explicit bounds keep storage predictable.

History pages are ascending, at most 100 messages and approximately 1 MiB.
`before`/`after` are exclusive global sequence cursors and mutually exclusive;
`start`/`end` are inclusive and ordered. `around` cannot combine with other bounds.
`limit` is 1–100, default 100. `message_id=ID` loads the page around a retained
message in the selected conversation, mutually exclusive with all sequence
bounds. Missing, removed and foreign-conversation IDs return `not_found`. This
lets a worker retrieve a quoted message beyond its recent context without
scanning intervening pages. Continue using the first/last sequence and
`has_before`/`has_after`. Global sequence gaps are normal. `content_revision` increases on sends,
edits, reactions, retractions and cleanup, independently of sequence. Compare it
in `/me`, history and conversation notifications. To reconcile edits/deletions,
rescan retained history and compare IDs/content with worker state; an `after` cursor
alone cannot detect them. Pass `content_revision` on every rescan page to pin one
revision; mismatch returns `conflict`, so restart the rescan. This is retained-state
reconciliation, not a durable event log; removed/expired content cannot be recovered. Concurrent changes can
return `conflict`; retry the page. Retention or cleanup can erase offline input.

`/read` advances the account's shared read cursor monotonically. Repeating an
already-advanced cursor succeeds even after its message expires. A new cursor
must identify a retained message. Worker processing checkpoints are separate
from read acknowledgements; never use `/read` as a processing acknowledgement. It clears the same unread
badge seen by humans logged into this account. The request requires explicit
`acknowledge_shared_unread:true` and restricted tokens need `read_ack` as well as
`read`; omit `read_ack` from ordinary worker credentials.

## WebSocket contract

The socket emits a baseline `snapshot`, then small
`{kind:"conversation",conversation_id}` notifications or replacement snapshots
for directory/permission/read/presence changes. It carries no durable event log.
Treat notifications as signals to fetch new history and compare content revisions. Catch
up on reconnect and while connected; deduplicate by sequence/message ID.
Snapshots are baselines, not instructions to replay every old mention.

Incoming text frames accept command discovery controls described below and
`{kind:"activity",conversation_id,state:"working"|"idle"}`. Activity requires send
access and the `activity` operation; readers of that conversation receive the
state, stable `account_id`, and an `expires` timestamp 30 seconds ahead. It is
transient, not persisted or replayed; refresh working state before expiry and
send idle on completion. Browsers with current conversation read access also
receive these signals. An optional `draft` string (at most 4,000 Unicode characters
and 16,000 UTF-8 bytes) carries provisional plain text. Browsers label it as a
generating draft, expire it after 30 seconds and remove it on idle/disconnection.
It never enters history, unread counts, Markdown rendering or retry receipts.
Workers must discover `ActivityRequest.properties.draft` in the schema before
sending it to older servers. SimpleBot throttles updates to at most two per second
and streams ordinary content only; tool arguments and reasoning are never shown. Accepted activity frames receive `config_ack` with
`id:null`. Activity is available to bot API sockets. Presence frames contain the
account-authorized online directory; conversation-restricted tokens omit this
account-wide directory. Chat
writes use HTTP. Ping/pong is handled by the WebSocket library: server pings
every 25 seconds and closes after 75 seconds without incoming traffic. Incoming frames
are at most 64 KiB; slow sends time out after five seconds. Revocation/disabling
closes affected sockets. Reconnect with backoff and check credentials on 401.

Configuration belongs to each worker. Use its advertised mention commands, such
as `@helper /config get` or `@helper /config set key "value"`; the worker
validates values and enforces its own owner/admin policy. `/bot config` is not a
server command. The socket RPC frames `config_ready`, `config_request` and
`config_response` are unsupported.
The `config` credential operation remains the gate for verified source context
used by private worker commands. Legacy `r:bot.config` / `w:bot.config` grants
remain readable in saved policies for compatibility but expose no remote RPC.

Accepted command/activity controls receive `{kind:"config_ack",id}` (the existing
acknowledgement name is retained); errors use `{kind:"error",id,error}`.

## Command discovery

Command discovery is part of Bot API v1. A worker registers its catalogue
handler with `{kind:"commands_ready"}` after receiving its baseline snapshot,
including when it has no commands. Observation-only sockets do not register.
This needs `read`, with `send`
required on conversations whose commands are advertised; it needs no operator
configuration grant or new credential operation. One command runtime owns an
account at a time. Repeating registration on that socket is safe.

The server sends `{kind:"commands_request",id,conversation_id}` for the worker's
readable, writable conversation IDs. Respond on the same socket with:

```json
{
  "kind": "commands_response",
  "id": "REQUEST_ID",
  "expires_in": 300,
  "commands": [
    {
      "name": "/model",
      "usage": "/model",
      "description": "Display the bot model settings"
    },
    {
      "name": "/history",
      "usage": "/history [mode]",
      "description": "Read history",
      "subcommands": [
        {
          "name": "before",
          "usage": "/history before <sequence>",
          "description": "Read older messages"
        }
      ]
    }
  ]
}
```

`expires_in` is an optional integer duration from 30 to 3,600 seconds, including
for empty catalogues. Omitted lifetimes default to 300 seconds for existing v1
plugins. The shipped workers also use 300 seconds. The server stores the validated catalogue until that
monotonic deadline and requests a replacement over the existing worker socket.
Socket maintenance notices expiry within 25 seconds. Expired catalogues are hidden
from reads until refreshed; unchanged metadata is not continually queried.

Return an empty array for no commands. Responses contain command metadata only,
never secrets or requester-specific private information. Set `operator_only:true`
on commands intended for the bot owner or global administrators; child commands
inherit a protected parent’s visibility. The browser endpoint filters these
commands using the viewer’s current authority. This is suggestion privacy, not
execution authorization: each worker defines and enforces its own command ACL. The
worker derives this list from its actual handlers, configuration and credential
capabilities without a model call. Command metadata does not authorize execution.
Send `{kind:"commands_changed"}` when settings change the catalogue; the server
invalidates old requests/cache and queries the current conversations again.
Directory and permission changes refresh discovery in the background, preserving
eligible catalogues and outstanding requests. Current credentials, conversation
access and operator visibility are still checked on every metadata read. Explicit
`commands_changed` cancels pending requests and removes the old catalogue before
rediscovery. Chat sends and history revisions do not trigger discovery. Registration/responses/changes receive the existing control
`config_ack` frame; errors retain the request ID when available.

Each request has a sixty-second deadline, at most eight in flight, and at most 256
conversations per runtime, selected in stable conversation-ID order. Each catalogue admits at most 64 command nodes total
and 32 KiB serialized JSON. Commands may contain `subcommands` arrays, up to three
levels including the root. Root names match `/[a-z0-9_-]{1,32}`; child names match
`[a-z0-9_-]{1,32}` without a slash. Names must be unique among siblings. Usage
starts with the full command path (for example `/history before`) followed by a
space or ends there. Usage/description are at most 256/512
UTF-8 bytes with no control characters. Description must be nonempty. A runtime
caches at most 256 KiB of catalogue JSON. The server admits at most 128 command
runtimes and 8 MiB of cached catalogue JSON across them. Replacements are checked
before publication; capacity rejection preserves the previous entry. Malformed, unsolicited, expired,
cancelled and wrong-socket responses are rejected. Responses to issued requests
do not consume the unsolicited control-frame budget. A rejected response leaves
its pending request available for correction until the original deadline; only
successful publication consumes the request. Async verification/inspection
failures retain the socket's discovery registration and catalogue state.

Timed-out requests are retried up to three attempts per conversation; routine
metadata changes do not continually cancel a slow response.

Caches live only with the authenticated socket. Reads recheck credential
expiry/revocation, current bot grants, and conversation visibility. Disconnect
removes the catalogue; reconnect registers and discovers again.

Completion descends through these subcommands locally; typing never queries the
worker. Selecting `/history` opens its modes, and selecting `before` inserts
`@BOT /history before ` while showing the sequence argument hint. A leaf's
arguments are free text. Metadata describes syntax; the worker still parses and
validates the ordinary message. Bot accounts need explicit private-conversation
grants for DM discovery and execution, just as for all other Bot API operations.

`GET /api/bot-commands` is the browser-session endpoint. Responses contain
`{conversations:[{conversation_id,bots:[{account_id,username,commands}]}],next}`.
Pages contain at most 32 conversations and target 512 KiB of serialized entries;
a single larger conversation is included for progress, bounded by the existing
128-publisher and 32-KiB-per-catalogue limits. Request `?after=NEXT` until `next`
is null to read the complete directory in stable conversation-ID order. Each entry applies live viewer and plugin
credential permissions; inaccessible conversations and protected commands are
filtered. Reading the cache does not send a plugin request. The optional legacy
`?conversation_id=ID` form still returns a single `{conversation_id,bots}`.
The browser fetches the complete paginated directory on connection, rechecks every 120 seconds, and
invalidates it on command notifications, membership or policy changes. Switching
conversations and typing reuse local metadata.

## Errors and limits

HTTP errors use `{error:{code,message,retryable}}`. Branch on `code`, not message
text. The same error object appears inside socket errors. `retryable` describes
an error category; only repeat operations whose recovery rule permits it.

| Code                              | HTTP                | Meaning                                                                                            |
| --------------------------------- | ------------------- | -------------------------------------------------------------------------------------------------- |
| `invalid_request`                 | 400/415/422         | Invalid fields, JSON, content type or bounds                                                       |
| `unauthorized`                    | 401                 | Missing/revoked/disabled credential                                                                |
| `forbidden`                       | 403                 | Current access or production ingress denied                                                        |
| `not_found`, `method_not_allowed` | 404/405             | Missing resource/route or wrong method                                                             |
| `conflict`                        | 409                 | Revision changed or room exists; rescan/retry desired-state operations, inspect creation conflicts |
| `idempotency_conflict`            | 409                 | Key reused with a different payload                                                                |
| `message_gone`                    | 410                 | Original send/new read position no longer retained                                                 |
| `payload_too_large`               | 413                 | HTTP body exceeds 32 KiB                                                                           |
| `rate_limited`                    | 429                 | Input budget exhausted                                                                             |
| `idempotency_capacity`            | 429                 | Receipt capacity exhausted                                                                         |
| `runtime_rejected`                | 400 (socket object) | Configuration handler already owned                                                                |
| `unavailable`                     | 5xx                 | Temporary worker/storage failure                                                                   |

Messages, reaction updates, read-cursor requests, credential issuance and conversation creation share 30 inputs per ten
seconds per account,
including retries and credentials. Control frames have a separate per-socket
30/ten-second budget. HTTP admission/socket limits are shared with the browser.
429 responses provide conservative `Retry-After` seconds: 10 for input budgets,
86,400 for receipt capacity. JSON numeric sequences/revisions fit safe integers.

For the runnable reference worker and checkpoint guidance, see
[Agent development](agent-development.md). Persistence/upgrade rules are in
[Data](data.md); checks are in [Testing](testing.md).

## Server commands, source context and ownership

Native server commands and a worker's own message commands are separate.
`GET /commands` returns server syntax allowed by the bot account's current command
grants. `POST /commands` executes that syntax as the bot, with current command and
action/target checks through the ordinary engine. It needs `read` and `commands`
credential operations and an account-wide conversation scope, because text
commands can explicitly name other targets. Bot-management, password/provisioning
and browser-only controls are excluded from this model-facing route. Room
creation, memberships, messages/reactions/history, renames and delegated grant
operations use the bot's ordinary grants. An admin bot has its corresponding
admin commands; a user claiming to grant access in a message changes nothing. The
reference worker forwards this catalogue unchanged as the model's command ACL and
refuses only commands outside it, so a conversation-scoped or `commands`-less
credential loses the tool entirely and the worker logs the denial.

Command POSTs require a durable `Idempotency-Key`, just like message sends, but
use a separate receipt namespace. The key is bound to the stable bot account,
conversation ID and command (outer whitespace trimmed). Repeating that request
returns the original result with `replayed:true`; another payload with the same
key returns `idempotency_conflict`. Use a new key for a new logical invocation,
including a fresh read-only query.

The server commits the command effect and serialized outcome in one SQLite
transaction. Nested room/account/message changes use savepoints. A rejected
command rolls back its effects and records its terminal error; a storage failure
rolls back without recording a terminal outcome. A lost response can therefore
be recovered by retrying the identical keyed POST or querying
`GET /commands/receipts/KEY`. Receipt states are `succeeded`, `failed`, or
`not_found`. `not_found` means no retained receipt, not proof that an expired
request never ran. Concurrent retries serialize and cannot duplicate the effect.

Receipts survive restart and renames, last 24 hours, and are bounded to 4,096
entries and 4 MiB of serialized outcomes per account, with at most 64 KiB per
outcome. Capacity rejects new requests rather than evicting live guarantees.
`created`/`expires` are epoch seconds. After expiry a reused key can execute again;
workers must never automatically replay an uncertain command outside that
window. Receipt lookup requires a currently valid, account-wide credential with
`read` and `commands`; revocation/disablement still deny access. A saved outcome
remains retrievable when the original target was removed or its grants changed,
so a successful destructive command does not become unknowable. Account deletion
clears that account's receipts. Outcomes can contain private command output and
belong in the same protected data backups as chat history.

The SimpleBot example saves intent/key/time before calling, retries the same key,
resolves pending keyed outcomes on restart, and feeds recovered results back to
the model. Legacy unkeyed tasks and commands near/outside receipt expiry require
inspection. Message-send receipts retain their separate existing guarantees.

Workers may query `GET /message-context` for a retained source message they can
read. This requires the credential's `read` and `config` operations in that
conversation. The source author is resolved from the stored `author_id`, never
from caller-supplied role or author names. The response is:

```json
{
  "author": {
    "id": "ACCOUNT_ID",
    "username": "alice",
    "groups": ["admin"],
    "permissions": null
  },
  "bot": {
    "id": "BOT_ID",
    "groups": [],
    "owner_id": "OWNER_ID",
    "owner": "alice"
  }
}
```

`permissions` is the author's effective action permissions in this conversation,
returned only when the bot itself has `r:policy.read` there; otherwise it is null.
The worker applies its own access policy using these trusted facts. The server
neither interprets nor implements the worker's private commands. simpleBot uses
owner/admin access and retains the protected-su boundary. Source/context lookups
recheck current credentials, identity, read access and content revision.

A registration persists an optional stable `owner_id` in its existing metadata.
New bots are owned by their human creator; stdin-created and legacy registrations
have no owner until a global administrator assigns `/bot owner BOT USER`.
`/bot info` reports the owner. Renames preserve ownership; deleting its account
clears it. Ownership authorizes only the bot's chosen private controls, not server
`/bot` management or credential issuance. The `/bot` family keeps its existing
server management grants and global administrator boundary.

## External worker context and responsibilities

The server owns identity, chat, grants, bounded history and durable write
receipts. Models, agent loops, schedules and external tools remain worker-owned.
Restarting a worker does not require a durable event stream: discard its model
context, fetch a recent history page, retrieve explicitly referenced
messages by `message_id`, and request more pages only when needed. Retention still
applies; no endpoint recovers deleted or expired content. Delivery keys and
processing/outbox records should survive restart to prevent duplicate effects.

The optional SimpleBot demonstrates this separation with a fixed system prompt,
separate configurable personality preferences, public `/help`, `/ping` and
`/metrics`, and owner/admin controls. It seeds context with 50 recent messages,
then appends stable history chunks for provider prefix caching. Cached rows are
revalidated and rebuilt when edited, removed or no longer retained. In-memory
context is bounded and discarded on restart; processing/outbox records survive.
`model.contextTokens` defaults to 32768 total input/output tokens and
`model.compactTokens` to 24576 input tokens. Compaction discards oldest
context and keeps history retrieval available, without an extra model call.
Provider-reported input counts are authoritative: unchanged requests use their
measured count, appended content adds only a new-suffix estimate, and actual
usage reaching the compaction threshold rebuilds the next answer's history.
Context rebuilds invalidate the baseline; new/rebuilt inputs and providers
without usable usage still need estimates. No extra model calls are made just
to count tokens. `/metrics` distinguishes measured counts from projected budgets,
alongside current-conversation usage/cache counters and reporting coverage;
missing telemetry is unknown, not zero. Short command
catalogues, duplicate suppression and bounded tool loops reduce unnecessary
calls without embedding any model runtime in the chat server.

Browser discovery failures retry with 30-second exponential rests capped at five
minutes, shared across catalogue-version changes within the same authority.
Successful directories, including empty ones, are cached for 120 seconds.
WebSocket catalogue notifications and changed conversation membership invalidate
the directory without resetting its outage backoff. Typing and ordinary renders
never bypass failed-request backoff.
