# Bot API v1

V1 is the chat transport for external agents under `/api/bot/v1/`.
This refactor changes the v1 contract; update workers and checkpoints together
with the server. The published schema and live contract tests define the API.
There are no unversioned bot routes. Native server command execution is provided
through the authenticated v1 `/commands` endpoint.

## Identity and provisioning

A bot is an ordinary account with a machine registration. The server owns
identity, grants and chat history; an external worker owns models, tools and
configuration. This keeps the API independent of any agent framework.

Global administrators, su or stdin manage registrations and account-wide tokens:

```text
/bot new helper #support
/bot token new helper worker
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
eight unique labels per account. They have the account's current permissions,
when issued through operator commands. Keep these provisioning credentials
with the operator. `POST /credentials` issues a worker token with mandatory
conversation IDs, operations and UTC epoch-second expiry within the next 24 hours:

```json
{
  "label": "worker",
  "expires": 1792000000,
  "conversations": ["CONVERSATION_ID"],
  "operations": ["read", "send", "react", "activity"]
}
```

Use a future expiry appropriate to the deployment. Operations are `read`, `send`,
`react`, `read_ack`, `activity`, `config`, and `commands`. Empty conversation scopes grant no
chat access. Scopes intersect the account's current grants; they never grant new
account authority. Restricted tokens cannot issue credentials, create conversations,
or authenticate browser/legacy transports. `/me` exposes the credential ID,
expiry, conversation scopes and operations. Revocation and expiry are checked on
requests and sockets; a heartbeat closes an idle expired socket within 25 seconds.
Tokens remain individually revocable through `/bot token rm`, with the existing
eight-token/unique-label limit. Expired entries must be removed before reusing their
label or capacity. Credentials survive restart in existing JSON metadata; no SQLite
schema migration is required. Never log credentials or message bodies.

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
fixed. `POST /conversations` uses an account-wide provisioning credential and existing
account permissions. Rooms require `/room` and `w:room.create`; direct conversations
require `/tell`, private send access, and `w:private.create` when the pair does not
already exist. Creation does not grant authority that the account lacks. Existing
direct pairs return the same ID without sending a message; room name collisions
return `conflict`. Bots created with room defaults need explicit operator grants
before initiating either kind. Creation is not automatically retried by the client.

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
validates values and enforces its own owner/admin policy. `/bot config` and the
`config_ready`, `config_request` and `config_response` socket RPC are removed.
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
conversations per runtime. Each catalogue admits at most 64 command nodes total
and 32 KiB serialized JSON. Commands may contain `subcommands` arrays, up to three
levels including the root. Root names match `/[a-z0-9_-]{1,32}`; child names match
`[a-z0-9_-]{1,32}` without a slash. Names must be unique among siblings. Usage
starts with the full command path (for example `/history before`) followed by a
space or ends there. Usage/description are at most 256/512
UTF-8 bytes with no control characters. Description must be nonempty. A runtime
caches at most 256 KiB of catalogue JSON. Malformed, unsolicited, expired,
cancelled and wrong-socket responses are rejected. Responses to issued requests
do not consume the unsolicited control-frame budget.

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

The browser reads cached metadata through session-authenticated
`GET /api/bot-commands?conversation_id=ID`, returning
`{conversation_id,bots:[{account_id,username,commands}]}`. It needs conversation
read/send/member-list access and includes only active participating bots with
current read/send credentials and grants. The browser socket emits
`{kind:"bot_commands",conversation_id}` when that cache changes. Selecting a
conversation, resuming focus, or receiving an invalidation refreshes this cache;
typing never queries the bot. The browser prefetches up to 32 conversations with
four simultaneous requests, prioritizing the selected conversation. Its in-memory
cache holds at most 32 conversations/512 KiB, survives selection changes and
transient refresh failures, and is discarded on logout, account/server-instance
or policy changes. Membership changes hide incompatible entries immediately;
authoritative access denials remove them. `@Bot /mo` completes to `@Bot /model ` in rooms
and DMs, retaining the explicit mention prefix. Bare slash commands remain
CommonRoom commands. Metadata contains no browser-executable code or action.

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
minutes. Empty successful catalogues stay cached until an invalidation or a focus
refresh after five minutes; populated focus refreshes rest for one minute.
WebSocket catalogue notifications and changed conversation membership can refresh
immediately. Typing and ordinary renders never bypass failed-request backoff.
