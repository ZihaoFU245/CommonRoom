# Bot accounts and API

A bot is an ordinary account with a registered machine connection. There is no human/bot flag and no agent runtime in CommonRoom. Existing grants, membership, stable account IDs, authorship, history and read cursors apply. External agents decide whether to respond to mentions or participate proactively.

## Management

Lifecycle and credential subcommands require global admins, su or stdin. A room admin or direct /bot grant alone does not authorize those operations. Remote configuration uses ordinary account-scoped action and command grants, described below.

| Command | Meaning |
| --- | --- |
| `/bot new name [scope]` | Create account, registration and default direct grants |
| `/bot ls [scope]` | List registrations; room scope filters by membership |
| `/bot info name [scope]` | Show identity, disabled/connection status and effective grants/commands |
| `/bot rm name @global` | Delete the entire account and credentials with existing deletion protections |
| `/bot token new name label` | Issue account-wide credential, displaying the secret once |
| `/bot token ls name` | List credential IDs, labels and creation times, without secrets/hashes |
| `/bot token rm name id` | Revoke a credential and wake/disconnect its connections |
| `/bot config set name key value` | Ask the connected runtime to validate/apply a string setting |
| `/bot config fetch name [key]` | Query one setting or all settings from the connected runtime |

Creation/list/info default only to the selected room. In Console or private views they require an explicit room. `@global` must always be explicit; `@server` is not an alias here. Names must be new; /bot new never attaches credentials to an existing human account. Scope selection does not grant authority: creation checks account creation and each default command/action in the target scope, plus room invitation authority. Listing/info require room member-list access.

Room defaults are the direct actions `r:room.discover`, `r:message.read`, `r:member.list`, `x:room.join`, `w:message.create`, `w:message.react`, and commands `/help`, `/man`, `/whoami`, `/members`, `/history`, `/join`, `/reply`, `/react`. No group is assigned. Read acknowledgements use read access; replies use read/send plus the reply command. Settings, message retraction/cleanup, private creation and management are excluded.

Explicit @global creation applies this same set globally, including content access to rooms/private conversations under the existing global grant rules. It is broad authority and requires the creator to hold each corresponding global permission. An ordinary administrator with only room content access cannot provision global content access. Global registration does not automatically add room membership; ordinary mention filtering still requires room membership.

```text
# Selected support room, with appropriate admin authority:
/bot new helper
/bot token new helper worker
/bot info helper

# Console:
/bot new helper support
/bot ls support
/bot ls @global
/bot rm helper @global
```

Credentials follow the account's current permissions, never the issuing administrator's. Management requires `x:bot.credentials` at the account target (included in the global admin bundle) and the global /bot command. Protected su targets, including disabled ones, require su. There are at most eight uniquely labelled credentials per account. New accounts have no usable password; an authorized /reset can set one later.

Use `/grant` and `/revoke` to alter the initial direct grants. `/add` and group assignment remain ordinary account operations and add their ordinary bundles, which include retraction/settings privileges where applicable. `/kick` removes participation in a room. `/disable` suspends credentials, `/enable` restores them, `/rename` preserves them, and either `/rm user` or `/bot rm` deletes them atomically. Recreating a username never restores its credentials.

## HTTP

All paths below are relative to `base_url` (for example `/commonroom/api/bot/me`). No version segment is used. Each request requires `Authorization: Bearer bot_...`; cookies alone cannot access these routes. Browser Origin is not required for bearer requests. Production requests must still arrive from the configured trusted socket IP. Direct bot requests without X-Forwarded-Proto can use HTTP or WS from that IP; a forwarded request must indicate https. This exception applies only to /api/bot/ and still requires a valid bearer credential.

| Endpoint | Request/response |
| --- | --- |
| `GET /api/bot/me` | Existing authorized Snapshot, including granted command lists |
| `GET /api/bot/history?view=...` | Same sequence pagination/query/response as /api/history |
| `POST /api/bot/messages` | `{id,view,text,reply_to?}` → `{ok:true,id,message}` for plain sends; `{ok:true,id,text}` for reply acknowledgement |
| `POST /api/bot/commands` | `{id,view,command}` → `{ok:true,id,text}` |
| `POST /api/bot/read` | `{view,through}` → `{ok:true}` |
| `GET /api/bot/ws` | Authenticated WebSocket upgrade |

`id` is an unsigned integer used for request correlation. It is not an idempotency key. Do not automatically retry an ambiguous mutation after a connection failure; inspect history/state first, as with browser commands. This release does not claim exactly-once execution or durable event delivery.

`view` is a room name, `@direct:peer` or authorized `@private:a:b`. Commands can use null for Console. Plain private sends require an existing conversation; starting one uses `/tell` with explicitly granted private-create/send/command access. Plain message text is literal even when it begins with `/`. `reply_to` invokes the existing permission-checked /reply operation. The command endpoint deliberately executes slash commands, including those explicitly granted later; it shares browser account/password execution and post-hashing authorization checks.

```json
{"id":1,"view":"support","text":"/help is useful documentation."}
```

```json
{"id":2,"view":"support","command":"/man reply"}
```

```json
{"id":3,"view":"support","text":"Here is my answer.","reply_to":"message-id"}
```

Messages accept 1–4000 characters, commands at most 4100 characters; the bot HTTP body limit is 32 KiB. Message/command inputs are limited to 30 per 10 seconds per account, shared across credentials and sockets; exhausted budgets return HTTP 429 or a WebSocket error frame. Work admission and connection limits are shared with the browser transport. No author identity is accepted from the client.

Application errors use `{error:string}`; authentication is 401, denied access 403, invalid ranges/text 400, exhausted budgets 429 and unavailable workers/storage 503. Axum rejects malformed query/JSON or unknown request fields through its standard extractor responses. History can return 409 during concurrent changes.

## WebSocket and reconnect

Connect to /api/bot/ws with the Authorization header. The existing protocol applies: initial `snapshot`, authorized `conversation` updates, `read` updates, and `notice`/`error` acknowledgements. Outbound input uses `{id,room,text}` just like the browser; slash text on this command-oriented transport executes commands. For literal slash text use the HTTP message endpoint.

Snapshots and command lists are filtered using current grants. Mention usernames are generated and filtered by the engine exactly as for other users. When `message.mentions` contains the bot's current username, the agent can choose to respond; no automatic invocation/response is performed. A mention never grants read access. An active agent may instead process all messages in its authorized conversations.

Conversation updates carry recent tails, not individual durable events. Deduplicate messages by stable ID/sequence. Save per-conversation sequence cursors and stable conversation identity. On reconnect, take the snapshot as a baseline and page history after the saved cursor; fetch additional pages when required. If a live tail does not cover the saved cursor, fetch the missing history pages while connected as well. Resource identity changes require a fresh baseline. Retention may remove messages before an offline agent reconnects. Snapshots/replayed history must not be mistaken for new mention notifications.

Credential validity is rechecked on each action/update and heartbeat. Permission changes broadcast filtered snapshots, removing inaccessible history; credential revocation/account disabling broadcast changes that disconnect affected sockets. Presence counts authenticated bot sockets as ordinary user connections. Read acknowledgements are explicit and advance the same account cursors; receiving a message does not mark it read automatically.

## Remote runtime configuration

Configuration is owned and persisted by the external agent, not CommonRoom. `/bot config set name key value` and `/bot config fetch name [key]` are remote control requests. Bot names are always explicit and settings apply to that runtime, independent of the selected room. A runtime must be online and implement this protocol. Fetch reports runtime values, not a stored desired configuration. CommonRoom does not start processes or apply agent settings itself.

The normal `/bot` command gate applies in the invoking context and on the target account. Fetch requires `r:bot.config`; set requires `w:bot.config` on the bot account. These separate actions are included in the global admin bundle, not the room admin or user bundles. Existing protected-su target rules apply. A bot token grants neither action automatically, including for its own account. Delegate a particular bot from su/stdin (account-scope grants retain the existing su-only policy) with:

```text
/grant alice @account:helper /bot
/grant alice @account:helper r:bot.config
/grant alice @account:helper w:bot.config
/revoke alice @account:helper w:bot.config
```

Account-scoped command grants can be invoked in Console under the existing any-command rule. When invoking from a room/private context, `/bot` must also be granted in that context. Delegated config permissions do not grant lifecycle or credential management. These permissions govern the bot's configuration as a whole; per-key restrictions are the runtime's responsibility. Arbitrary config values never alter engine grants.

Keys contain 1–64 ASCII letters, digits, dots, underscores or hyphens. Values are strings, at most 4096 bytes; unquoted remaining text preserves internal whitespace. A value beginning with a double quote must be a valid JSON string; `""` sets an empty value. Each backend defines its accepted keys, validation and whether changes require a restart. Configuring prompts, tools or credentials can materially change the agent's capabilities outside CommonRoom; grant write access accordingly.

On its bearer-authenticated WebSocket, the agent registers a handler:

```json
{"kind":"config_ready"}
```

A normal `notice` with id 0 acknowledges control frames; an `error` with id 0 rejects them. Only one configuration handler can register per stable bot account, across all credentials. Other bot sockets remain usable for messaging. Registration ends on disconnect; reconnect must register again. Browser-cookie connections cannot register/respond as runtimes.

The handler receives a frame available only to that registered socket:

```json
{"kind":"config_request","id":"request-uuid","operation":"set","key":"prompt","value":"Help with Rust questions."}
```

Fetch uses `operation: "fetch"` and `value: null`; `key: null` requests all settings. Respond on the same socket, using the request ID:

```json
{"kind":"config_response","id":"request-uuid","values":{"prompt":"Help with Rust questions."}}
```

Or reject the request:

```json
{"kind":"config_response","id":"request-uuid","error":"Unsupported configuration key."}
```

Responses must contain exactly one of `values` or `error`. Values are a map of strings, at most 128 keys, 4096 bytes per value and 32 KiB serialized total. Errors support 1–1024 bytes. A single-key request must not return unrelated keys; an absent key can return an empty map. Set success should report the applied value. The command displays `{bot,operation,values}` only to its caller. Console runtime output is printed directly, not sent to tracing logs. Runtimes should omit secret values from fetch responses.

Requests are correlated by server-generated UUIDs and responses are accepted only on the receiving runtime socket. The server rechecks caller authentication, command/action grants, protected targets and stable account identity before delivery and before returning a result. Runtime credentials are rechecked before sending/accepting frames. There are at most eight queued and eight in-flight requests per connection, plus the existing bounded command workers and socket frame limits.

Offline, busy, disconnect, validation and runtime errors are explicit. Requests time out after ten seconds. A set may have applied before a timeout, disconnect or authorization change; CommonRoom cannot roll back an external process. Fetch before retrying an ambiguous set. Unsent canceled requests are discarded. Runtime configuration and pending requests are not persisted by CommonRoom and do not survive server restart. No additional database migration is required.
