# Bot API v1 — frozen

V1 is the stable, lightweight chat transport for external agents. Its endpoints,
fields, error codes, limits and recovery rules are frozen. `/api/bot/v1/` is the
sole bot API namespace; no alternative version is planned. Implementation and
security fixes must preserve this contract. The schema fingerprint test enforces
this policy. There are no unversioned routes or free-form bot command endpoint.

## Identity and provisioning

A bot is an ordinary account with a machine registration. The server owns
identity, grants and chat history; an external worker owns models, tools and
configuration. This keeps the API independent of any agent framework.

Global administrators, su or stdin manage registrations and account-wide tokens:

```text
/bot new helper support
/bot token new helper worker
/bot info helper support
/bot token ls helper
/bot token rm helper CREDENTIAL_ID
/bot rm helper @global
```

Creation requires a new name and the creator's account-create, room-invite and
corresponding content/command grants. A selected room supplies the default scope;
Console/private contexts require an explicit room. `@global` must be explicit
and grants broad room/private content access. Room admins alone cannot provision
or issue credentials. Su accounts remain protected from ordinary admins.

Room defaults grant discover/join/read/member-list/send/react and
`/help`, `/man`, `/whoami`, `/members`, `/history`, `/join`, `/reply`, `/react`.
There is no group assignment, password, private-create or management permission.
These command grants support existing engine authorization; v1 exposes only the
HTTP operations below. `/grant` and `/revoke` adjust access. `/kick` removes room
participation; `/disable` suspends tokens; rename preserves stable IDs; account
deletion removes tokens and retry receipts. Name reuse restores no old authority.

Tokens are shown once, hashed at rest and individually revocable, with at most
eight unique labels per account. They have the account's current permissions,
with no independent token scope or expiry. Use separate accounts for different
permission boundaries. Never log credentials or message bodies.

## HTTP contract

All routes are relative to the configured `base_url`. Each requires
`Authorization: Bearer bot_...`; cookies alone do not authenticate bots.
Responses include `Bot-Api-Version: 1`. Use HTTPS/WSS through the configured
proxy. Production always requires the trusted socket-peer IP. Direct HTTP/WS
without `X-Forwarded-Proto` is allowed only from that IP under `/api/bot/v1/`;
forwarded requests must indicate HTTPS. A browser Origin is unnecessary.

| Method and endpoint | Payload / result |
| --- | --- |
| `GET /api/bot/v1/me` | `Snapshot`: account and readable conversation directory |
| `GET /api/bot/v1/history` | `conversation_id`, sequence bounds and `limit` query; `HistoryResponse` |
| `POST /api/bot/v1/messages` | `MessageRequest` plus required `Idempotency-Key`; `SendResponse` |
| `POST /api/bot/v1/read` | `{conversation_id,through}`; `{ok:true}` |
| `PUT /api/bot/v1/reactions` | `{conversation_id,message_id,reaction,active}`; `{ok:true}` |
| `GET /api/bot/v1/ws` | Authenticated notification/configuration socket |
| `GET /api/bot/v1/schema` | The published JSON Schema |

The [schema](bot-v1.schema.json) is normative for shapes and types. Validate the
named `definitions` entry for each payload, or `ServerFrame` / `ClientFrame` for
socket directions. Unknown request fields are rejected. UTF-8 byte bounds,
permissions and history bound combinations have additional server checks.
HTTP has no request-correlation `id`; its response belongs to the request.

A snapshot contains `{kind:"snapshot",account:{id,username},conversations}`.
Each conversation has stable `id`, current display/routing label `view`,
`permissions:{send,reply,react}`, filtered `members`, and `unread` metadata.
The directory contains only readable conversations. An empty member list can
mean member-list access is absent. The snapshot carries no message tails,
command catalogue or browser settings.

Use the **conversation ID** in every operation. IDs are opaque strings of 1–128 ASCII letters, digits, dots, underscores, colons or hyphens;
never derive them from names. Rename preserves an ID. Delete/recreate uses a
new ID, so an old request cannot be redirected into a replacement conversation.
Existing private conversations work the same way; their participants remain
fixed. Provision/join/create private conversations through authorized operator
commands, outside the frozen machine API.

## Messages and reactions

Send `{conversation_id,text,reply_to?,markdown?}` with a fresh idempotency key.
Text is trimmed, supports 1–4,000 Unicode characters, and remains literal even
when it starts with `/`. Markdown defaults to false. A reply needs current read,
send and `/reply` grants; all v1 sends need read/send access to return the result.
A reply target must be retained in the selected conversation.

Both plain sends and replies return `{message,replayed}`. A message contains
stable `id` and `author_id`, current author label `from`, text/format/edit marker,
UTC epoch-second `time`, global `sequence`, nullable reply quote, filtered
mention usernames and reactions. Reply quotes capture at most 160 characters.
There is no automatic invocation: an agent chooses how to act on `mentions`.

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
`limit` is 1–100, default 100. Continue using the first/last sequence and
`has_before`/`has_after`. Global sequence gaps are normal. Concurrent changes can
return `conflict`; retry the page. Retention or cleanup can erase offline input.

`/read` advances the account's shared read cursor monotonically. Repeating an
already-advanced cursor succeeds even after its message expires. A new cursor
must identify a retained message. Worker processing checkpoints are separate
from read acknowledgements; mark read only when appropriate for your agent.

## WebSocket contract

The socket emits a baseline `snapshot`, then small
`{kind:"conversation",conversation_id}` notifications or replacement snapshots
for directory/permission/read/presence changes. It carries no durable event log.
Treat notifications as signals to fetch history after your saved cursor. Catch
up on reconnect and while connected; deduplicate by sequence/message ID.
Snapshots are baselines, not instructions to replay every old mention.

Incoming text frames accept only `config_ready` and `config_response`. Chat
writes use HTTP. Ping/pong is handled by the WebSocket library: server pings
every 25 seconds and closes after 75 seconds without incoming traffic. Incoming frames
are at most 64 KiB; slow sends time out after five seconds. Revocation/disabling
closes affected sockets. Reconnect with backoff and check credentials on 401.

Optional runtime configuration is worker-owned. An authorized operator uses
`/bot config set helper key value` or `/bot config fetch helper [key]`.
Fetch/set need `/bot` context/target grants and account-scoped `r:bot.config` /
`w:bot.config`; room administration and a bot's own token do not supply them.
Account-scope delegation is su-only. The worker validates/persists values and
omits secrets from fetch results.

Register `{kind:"config_ready"}`; repeating it on the same socket is safe. Only
one handler may own an account across credentials. Reconnect must register again.
The handler receives `{kind:"config_request",id,operation,key,value}` with an
opaque server request ID. Operations are `set` or `fetch`; fetch has `value:null`
and `key:null` means all settings. Respond on that socket with
`{kind:"config_response",id,values:{...}}` or `{kind:"config_response",id,error}`,
with exactly one non-null result. Single-key responses cannot include other keys.

Accepted control frames receive `{kind:"config_ack",id}`; registration has
`id:null`. Rejections use `{kind:"error",id,error:{code,message,retryable}}`.
Keys use 1–64 ASCII letters, digits, dots, underscores or hyphens. Responses have
at most 128 string values, 4,096 UTF-8 bytes each and 32 KiB serialized total.
Error text is 1–1,024 UTF-8 bytes. Requests have a ten-second timeout, at most eight
queued and eight in flight per handler. A set may apply before a timeout; fetch
before retrying. Configuration and pending RPCs do not survive server restart.

## Errors and limits

HTTP errors use `{error:{code,message,retryable}}`. Branch on `code`, not message
text. The same error object appears inside socket errors. `retryable` describes
an error category; only repeat operations whose recovery rule permits it.

| Code | HTTP | Meaning |
| --- | --- | --- |
| `invalid_request` | 400/415/422 | Invalid fields, JSON, content type or bounds |
| `unauthorized` | 401 | Missing/revoked/disabled credential |
| `forbidden` | 403 | Current access or production ingress denied |
| `not_found`, `method_not_allowed` | 404/405 | Missing resource/route or wrong method |
| `conflict` | 409 | History changed; retry its page |
| `idempotency_conflict` | 409 | Key reused with a different payload |
| `message_gone` | 410 | Original send/new read position no longer retained |
| `payload_too_large` | 413 | HTTP body exceeds 32 KiB |
| `rate_limited` | 429 | Input budget exhausted |
| `idempotency_capacity` | 429 | Receipt capacity exhausted |
| `runtime_rejected` | 400 (socket object) | Configuration handler already owned |
| `unavailable` | 5xx | Temporary worker/storage failure |

Messages and reaction updates share 30 inputs per ten seconds per account,
including retries and credentials. Control frames have a separate per-socket
30/ten-second budget. HTTP admission/socket limits are shared with the browser.
429 responses provide conservative `Retry-After` seconds: 10 for input budgets,
86,400 for receipt capacity. JSON numeric sequences/revisions fit safe integers.

For the runnable reference worker and checkpoint guidance, see
[Agent development](agent-development.md). Persistence/upgrade rules are in
[Data](data.md); checks are in [Testing](testing.md).
