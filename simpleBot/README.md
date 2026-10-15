# simpleBot — a Bot API v1 reference worker

A single-file-per-concern TypeScript worker for CommonRoom's Bot API v1. It is a
**zero-dependency** example: Node 22.18+ runs `src/main.ts` directly with native
type stripping, and Node's built-in `WebSocket` sends the `Authorization` header
the bot socket handshake needs. No `npm install`, no build step, no agent
framework. Deno 2.5+ can also run it using the included `deno.json` template.

From `simpleBot/` (or its `data/` subdirectory):

```sh
deno task config model.name "ds-zf/deepseek-flash"
deno task start
# Restart when source files change, keeping logs visible:
deno task dev
```

The default permission profile permits reads/writes only under `./data`, plus
network and environment access. Add any external `CHAT_DATA`, `BOT_CONFIG` or
`BOT_STATE` paths to its read/write permissions if overriding those locations.
Permission profiles are experimental in Deno and may print a warning. Model and
chat connection settings remain in `data/config.json`, not in `deno.json`.

It is deliberately more than a "hello world" reply loop. One bot demonstrates
the core chat features the v1 surface offers, so the code doubles as executable
documentation:

| Feature                                                    | Where                                                |
| ---------------------------------------------------------- | ---------------------------------------------------- |
| Snapshot catch-up with per-conversation cursors            | `src/agent.ts` (`syncOnce`, `advance`)               |
| Keyed idempotent sends with a durable outbox               | `src/agent.ts` (`sendParts`), `src/store.ts`         |
| Multi-part output over the 4,000-character limit           | `src/api.ts` (`chunkMessage`), `sendParts`           |
| Edit/retraction handling via `content_revision`            | `src/agent.ts` (`reconcile`)                         |
| Revision-pinned history rescans (`409 conflict` → restart) | `src/api.ts` (`history`)                             |
| Replies, mentions, desired-state reactions                 | native command tools and typed `BotApi.react`        |
| Transient `working`/`idle` activity signals                | `beginActivity`, `setActivity`                       |
| Owner/admin configuration through chat commands            | `src/agent.ts`, `src/config.ts`                     |
| Secret redaction in configuration fetches                  | `ConfigStore.fetch`                                  |
| Presence directory (account-wide credentials)              | `src/main.ts`, private status reports                |
| Shared-unread acknowledgement behind explicit opt-in       | `acknowledge_read` tool, `chat.acknowledgeReads`     |
| Credential scopes and expiring tokens                      | `/status`, `snapshot.credential`                     |
| Conversation creation (rooms and DMs)                      | native `/new room`, `/tell` and typed client methods |
| Hardened checkpoint (atomic write, version refusal)        | `src/store.ts`                                       |
| Bounded retries, `Retry-After`, typed error codes          | `src/api.ts`                                         |

## Files

```
simpleBot/
├── src/
│   ├── main.ts        entrypoint: config, transport, lifecycle, logging
│   ├── agent.ts       catch-up, revision reconciliation, commands, output
│   ├── context.ts     append-only history prefix and on-demand pages
│   ├── tokens.ts      provider token baselines and bounded context compaction
│   ├── metrics.ts     numeric model/cache diagnostics per conversation
│   ├── api.ts         typed HTTP client, idempotency, chunking, retries
│   ├── socket.ts      WebSocket notifications, discovery/activity frames, reconnect
│   ├── wiring.ts      socket event handlers (shared by the entrypoint and tests)
│   ├── config.ts      runtime settings, validation, redaction
│   ├── store.ts       checkpoint: cursors, revisions, task ledger, outbox
│   ├── model.ts       OpenAI-compatible chat client
│   ├── wire.ts        v1 wire types transcribed from the published schema
│   ├── log.ts         shared logging contract
│   └── config-cli.ts  inspect or seed the local config file
├── test/
│   ├── mock-server.ts in-process Bot API v1 stand-in
│   ├── agent.test.ts  worker and checkpoint regression tests
│   ├── config.test.ts JSON/CLI persistence and runtime configuration
│   ├── context.test.ts context budget and history retrieval
│   ├── transport.test.ts HTTP, socket and model streaming regressions
│   └── integration.test.ts actual worker against an isolated Rust server
├── tsconfig.json
└── package.json
```

## 1. Provision the bot (server side)

Global admin, su, or the stdin console:

```text
/new room support
/bot new helper support
/bot token new helper worker
```

The token is shown once. Keep that secret aside; it is the **provisioning**
credential.

Optional, and needed only if you want the bot to start conversations on its own:

```text
/grant helper @global w:room.create
/grant helper @global /new
/grant helper @global w:private.create
/grant helper @global /tell
```

Room defaults already cover read/send/reply/react. `/bot new` delegates only the
permissions you hold yourself, so grant from a suitably privileged account.

## 2. Start the worker

```sh
BOT_TOKEN=bot_... node simpleBot/src/main.ts
```

`BOT_TOKEN` is the credential; `CHAT_URL` (default `http://127.0.0.1:3000/`)
is the base URL, including the hosting base path when the server is not at the
root. Both are read from the environment and are **not** written to the config
file, so a deployment can inject the secret and an operator's stored settings are
never clobbered.

The worker starts even with no model configured: it connects, subscribes, and
waits for configuration.

## 3. Configure it at runtime

```text
@helper /config set chat.url "https://chat.example.com/"
@helper /config set model.endpoint "https://ai.snowy-richter.ts.net/"
@helper /config set model.name "deepseek-flash"
@helper /config set model.reasoningEffort "none"
# This endpoint needs no model.token; leave it empty.
@helper /config get
```

`@helper /config get` returns every key with secrets redacted as
`<set>`. Changing `chat.url` or `chat.token` reconnects the worker with
the new transport immediately. Values are validated on arrival; an invalid value
is rejected with a reason and leaves the previous setting untouched, for example:

```text
@helper /config set model.timeoutMs "soon"
# → rejected: timeout must be an integer 1000-600000 ms
```

### Recognized keys

| Key                     | Default                  | Notes                                                                        |
| ----------------------- | ------------------------ | ---------------------------------------------------------------------------- |
| `prompt`                | _(empty)_                | Optional personality and user preferences; up to 4096 UTF-8 bytes            |
| `chat.url`              | `http://127.0.0.1:3000/` | the only preset value; include the base path, omit `/api/bot/v1`             |
| `chat.token`            | _(from `BOT_TOKEN`)_     | bot credential; redacted on fetch                                            |
| `model.endpoint`        | _(empty)_                | OpenAI-compatible base URL or a full `/chat/completions` path                |
| `model.token`           | _(empty)_                | optional bearer key; omitted entirely when empty                             |
| `model.name`            | _(empty)_                | model id sent to the endpoint                                                |
| `model.timeoutMs`       | `120000`                 | 1000–600000                                                                  |
| `model.stream`          | `true`                   | streamed completions                                                         |
| `model.maxTokens`       | `4096`                   | output budget including reasoning; 128–32768                                 |
| `model.reasoningEffort` | _(empty)_                | provider default when empty; none, low, medium or high if supported          |
| `autonomous`          | `false`                  | Participate in writable rooms without mentions, using fixed 3-second batches |
| `chat.respondToAllDms`  | `true`                   | answer every direct message, not only mentions                               |
| `chat.acknowledgeReads` | `false`                  | enables the read acknowledgement tool                                        |
| `chat.historyWindow`    | `50`                     | prior messages requested for model context, within the character budget      |
| `chat.contextChars`     | `131072`                 | Additional history character ceiling; 0–524288, 0 disables history           |
| `model.contextTokens`   | `32768`                  | Total input/output token budget; measurements plus estimates for new content |
| `model.compactTokens`   | `24576`                  | Input threshold for context compaction; less than total                      |
| `model.maxToolCalls`    | `16`                     | Maximum tool calls per turn; 1–16                                           |
| `chat.stateLimit`       | `200`                    | 10–5000 retained task records                                                |
| `log.level`             | `info`                   | `debug`, `info`, `warn`, `error`, `silent`                                   |

The model client uses the standard OpenAI-compatible text Chat Completions
format, with optional bearer authentication and streamed or buffered responses.
It is provider-neutral, but does not guarantee every provider/model: custom
authentication, different API formats or incompatible request parameters need
an adapter. Commands are handled locally and do not depend on model support.

Reasoning models can spend the output budget before generating answer text.
`model.maxTokens` defaults to `4096` (range `128–32768`).
`model.reasoningEffort` defaults to empty, which omits `reasoning_effort` from
requests; supported values are `none`, `low`, `medium`, and `high`, depending
on the endpoint. The Aperture endpoint above accepts `none`, which keeps this
demo fast. A token-limit response fails with an explicit diagnostic and is not
retried with the same budget. Reasoning text is never posted as an answer.

`node simpleBot/src/config-cli.ts` reads or seeds the same file directly when you
have shell access: `node simpleBot/src/config-cli.ts chat.url http://host:3000/`.

The chat URL and worker behavior have defaults. `config.json` is **created on first run** with those
defaults; the provider endpoint and model ID start empty, so prose questions fail with a clear
message until an endpoint and model id are supplied. `BOT_TOKEN` and `CHAT_URL`
seed in-memory values without being written to the file; explicit runtime sets replace those seeds. Check what is in effect
with `node simpleBot/src/config-cli.ts`.

The system prompt stays fixed in code, including identity, Markdown formatting
and server tool rules. The optional `prompt` setting supplies personality and
user preferences as a separate user message, never as system instructions.

Chat control values must be double-quoted JSON strings, including numbers and
booleans. For example:

```text
@helper /config get
@helper /config set prompt "Use Markdown and give concise examples"
@helper /config set model.stream "false"
@helper /config set model.token ""
@helper /reload
```

Use `\"` for a literal quote and `\n` for a newline inside a chat value.
`/config get` (also `get .`) prints all settings as readable JSON; credentials
remain redacted. Changes take effect immediately. `/reload` or SIGHUP loads
external file edits, validates the entire document, and preserves live settings
if anything is invalid. Model requests already in progress finish with their
original settings; later requests use the new settings.

The CLI accepts one shell-quoted value argument, for example
`node simpleBot/src/config-cli.ts prompt "Use Markdown with examples"`.
Its output is valid JSON. It validates the current file before writing, merges
other writers' saved keys, and uses a private, flushed temporary file plus atomic
rename. Concurrent writers fail clearly with a busy error instead of losing an
update. If a writer is killed, stop config writers before removing its stale
`config.json.lock` file. Invalid existing JSON is rejected and never replaced
with defaults.

## 4. Talk to it

Mention the bot in a room; ordinary text in a DM is answered when
`chat.respondToAllDms` is enabled. The model receives the server's current command
catalogue and native function tools. For example, `@helper create a room named
study` can execute `/new room study` as **helper**, when helper has both command
and action grants. Granting the bot admin access enables its corresponding
server commands. The server rechecks grants on every execution; the requesting
user's permissions are not borrowed. Tool-capable Chat Completions models are
required for actions. Streaming and buffered function calls are supported.

Bot-specific controls are a separate namespace, implemented and authorized by
the worker. They are available only to the bot's owner or a global administrator:

| Message                           | Purpose                                                 |
| --------------------------------- | ------------------------------------------------------- |
| `@helper /admin`                  | List this bot's private controls                        |
| `@helper /whoami`                 | Bot identity and owner                                  |
| `@helper /owner`                  | Bot owner                                               |
| `@helper /status`                 | Runtime state, scopes, cursors and counters             |
| `@helper /features`               | Worker implementation features                          |
| `@helper /model`                  | Model configuration                                     |
| `@helper /config get [key]`       | Show every key/value pair, or one key; secrets redacted |
| `@helper /config set key "value"` | Update any setting immediately                          |
| `@helper /reload`                 | Validate and reload the local configuration file        |

The server verifies the source message's author and returns current role and
ownership facts through `GET /message-context`; the worker applies its own
owner/admin policy. Claimed roles in message text grant nothing. Configuration
reports and command definitions are excluded from model context. Changing transport
credentials/destinations reconnects after the acknowledgement. Environment seeds
remain in memory unless explicitly replaced by a runtime set.
Use `@helper /status`, `@helper /model` or `@helper /features` for worker reports.

`/bot ...` manages bot accounts, credentials and ownership. New registrations
record their creator as owner; stdin-created and legacy registrations default to
administrator control until assigned with `/bot owner helper alice`. Ownership
uses stable account IDs, follows renames and is cleared if its account is deleted.
An owner does not gain server bot-management or token-management privileges.

Discovery advertises the worker's own controls with `operator_only:true`.
CommonRoom filters private suggestions using current owner/admin facts; the
worker still checks authorization at execution. Owners/admins can type
`@helper /mo` then Tab for `/model`, or `@helper /config ` to choose `get`/`set`.
Other readers do not receive that private catalogue. Completion is cached and
never calls the model. The model's server-command catalogue is separate and
contains no worker controls.

## Try the revision path

Edits and retractions preserve message `sequence`, so they are invisible to a
cursor alone. That is what `content_revision` is for:

1. Ask the bot something: `@helper what is 2+2?`
2. Edit your own message to a different question (the UI's Edit action).
3. The bot logs `revision N -> M`, rescans pinned to the new revision, detects
   that the stored text hash changed, and answers the **revised** question in a
   separate numbered `:edit:N` slot. The original answer stays in the transcript.

`/retract` works the other way: the message disappears from retained history, the
task is cancelled, and any unsent parts are dropped.

## Testing

```sh
node --test simpleBot/test/*.test.ts
# After building the Rust server: real HTTP/WS and entrypoint lifecycle
BOT_INTEGRATION=1 node --test simpleBot/test/integration.test.ts
node simpleBot/src/main.ts            # run it
pnpm --dir simpleBot check           # typecheck with the project dev tools
```

The unit tests use an in-process loopback HTTP server and a stub model, with no external server or credentials. The opt-in integration test launches an isolated Rust server, the actual worker process, and a keyless local model gateway under a hosting base path. They cover first-run baselining, idempotent replay, additive edit
passes, an edit arriving mid-answer, retraction cancellation, local commands,
unknown-command passthrough, chunking, reactions, the `/read` opt-in, credential
scope failures, configuration round-trips and rejection, checkpoint durability
and version refusal, cross-account refusal, activity signals, and the
`unread: null` deferral contract.

## Operational notes

- **Checkpoint**: `simpleBot/data/checkpoint.json`, mode 0600, atomic rename. It
  holds the per-conversation input cursors, the last reconciled
  `content_revision`, the per-input task ledger, and the pending-output outbox.
  This is what makes a restart safe: output is recorded before it is sent and
  the cursor only advances after that output is acknowledged, so an interrupted
  answer replays with the same idempotency key instead of posting a duplicate.
  An empty file (`account: null`, no cursors) means the worker never saw a
  readable conversation — check the credential and the log line above.
  `BOT_STATE` overrides the path; `CHAT_DATA` moves both defaults. One worker owns
  one checkpoint; the file records the account id and refuses to run against a
  different one.
- **Checkpoint version 3**: version 2 files upgrade automatically; version 1 files are refused, because
  adopting the new revision watermarks without reconciling can duplicate output.
  Reconcile the old file deliberately before replacing it; deleting it discards any pending delivery guarantees.
- **`/read` is not a processing acknowledgement.** It clears the human-facing
  unread badge for the whole account. It is off by default, needs
  `chat.acknowledgeReads`, and needs `read_ack` on the worker credential. Leave
  `read_ack` out of worker tokens.
- **Account-wide tokens**: `POST /credentials` and
  `POST /conversations` require an account-wide provisioning credential, so a
  restricted worker token cannot issue credentials or create conversations. Use
  the provisioning token with `curl` for those, or give the worker an
  account-wide token if it genuinely needs that authority.
- **Retries are safe for delivery, not for effects.** A chat receipt means the
  message is deduplicated; your own tool calls, writes and payments are not.
  `taskId(slot)`-style stable ids (`conversation:message:slot`) are the hook for
  making an external effect idempotent.
- **Logs omit message bodies and credentials.** Preview text only appears in the
  transcript itself. Raise `log.level` to `debug` to see conversation ids and
  revisions.
- **Markdown.** Bot API sends default to Markdown; `markdown:false` opts out. The model prompt asks for Markdown and fenced code blocks.
- **Catch-up.** Notifications are coalesced; an additional check every 30 seconds recovers after missed signals or failed fetches.
- **Standby when the server is offline.** The worker retries quietly with backoff
  up to 30 s, logs the state change once, and resumes on its own when the server
  returns. A long outage stays silent.
- **Activity compatibility.** The worker checks the server schema once per connection and only emits activity when advertised and permitted by its credential. Older servers that accept only configuration frames keep working without repeated activity errors. Rebuild the server to enable activity.
- **Reconnects are normal.** The server pings every 25 s and closes after 75 s
  without incoming traffic; the worker reconnects with exponential backoff to
  30 s.
- **Scoped credentials do not receive presence**. Generic text command execution also requires an account-wide conversation scope; typed chat/history/reaction operations retain their existing credential scope checks.

## Extending it

`onMessage`-equivalent behaviour lives in `Agent.answer`. Replace `askModel` with
your own orchestration, or add a private control in `WORKER_COMMANDS` and its handler. Keep the transport
rules intact:

1. Persist the output request (payload and idempotency key) **before** sending.
2. Never advance the input cursor before the output is acknowledged.
3. Give each intended output a stable slot name so recovery reuses it.
4. Treat `content_revision` as "something in retained history changed" and rescan
   with the revision pinned; do not assume it means an edit.

The server-side contract is in [`docs/bots.md`](../docs/bots.md), the normative
schema in [`docs/bot-v1.schema.json`](../docs/bot-v1.schema.json), and worker
guidance in [`docs/agent-development.md`](../docs/agent-development.md).

Native commands use separate durable server receipts. The worker saves command
intent/key/time before execution, suppresses repeated identical calls, and retries
only the identical key within the receipt window. On restart it recovers pending
keyed outcomes before asking the model to continue. Legacy unkeyed or expired
uncertain actions require inspection. The v3 checkpoint accepts these optional
fields without replaying old unkeyed effects. Run one worker per checkpoint/account.

Model history uses a bounded `read_history` tool that filters worker control messages and their responses, matching the normal context filter. Raw `/history` model calls are redirected to that tool.

## Bounded history and low-token operation

Public `@helper /help`, `@helper /ping` and `@helper /metrics` are available to ordinary room members
and use no model calls. `/admin` lists the separate owner/admin controls; other
members do not receive that private catalogue.

History context stays in memory, in chronological chunks that grow by appending.
The initial baseline requests 50 recent messages; later answers append intervening
messages instead of sliding the prefix on every answer. Before reuse, the worker
revalidates cached messages against retained server history, rebuilding on edits,
removal or retention. No model conversation is checkpointed. Restart discards
that context and the next answer fetches retained history again.
Delivery keys, pending command intent/results and input checkpoints remain durable
so restart cannot duplicate committed effects. The default total budget is 32768
tokens, including the requested output allowance; compaction starts at 24576
input tokens. Compaction rebuilds a recent baseline or removes oldest
history chunks, retaining the fixed instructions and current input. Large tool
results can be explicitly truncated while preserving call IDs and results;
compaction never repeats a tool effect and uses no extra model call. Older
history remains retrievable. The additional history character ceiling is 131072.
Provider-reported `usage.prompt_tokens` is the authoritative measured input
count, including cached input. An unchanged request reuses that exact count.
Appended requests use the measured baseline plus a UTF-8 estimate of new suffix
content. Small replaced suffixes are not subtracted using invented exact counts;
the projection stays conservative. Changed tool definitions/reasoning settings,
large prefix changes, context rebuilds and restart invalidate the baseline.
Only hashes and byte sizes of measured messages are kept, scoped to at most 16
conversations. Missing usage preserves an earlier usable baseline, otherwise the
worker falls back to a full estimate calibrated upward from provider reports.
An actual reported count at or above the compaction threshold rebuilds the next
answer's history; tool rounds also check measured baselines before continuing.
No counting-only model requests are sent. New or rebuilt content still needs an
estimate before sending, so this is not an exact preflight tokenizer guarantee.
Output is capped to the projected remaining total budget; oversized fixed/input
content fails before sending. Cached prefixes are bounded to 16 conversations
with least recently used eviction.
It sends compact message rows rather than complete API message objects and keeps
command usage strings without repeating their descriptions.

Reply to an older message, or include a message UUID, to automatically fetch its
page before recent context. The `read_history` model tool also accepts
`message_id`, `around`, `before`, `after` and `limit` (at most 20); choose one
cursor mode. Results include page boundaries and `has_before`/`has_after`.
The model can decide to request more pages. Repeated pages/text are suppressed;
truncated messages can continue in later retrievals with a character offset.
Removed/expired messages stay unavailable. Private maintenance transcripts are
filtered from both initial context and tool retrieval.
Removing an input aborts its in-flight model request so the next queued mention
can proceed without waiting for the removed request's timeout.

`chat.historyWindow`, `chat.contextChars`, `model.contextTokens`,
`model.compactTokens` and `model.maxToolCalls` tune these
bounds live. Four tool calls are allowed by default; output `model.maxTokens`
remains a configurable upper bound, not a request to fill that budget. This
reduces input and redundant calls without claiming an exact tokenizer-independent
token minimum. Personality/preferences remain a separate user message and never
replace the fixed system instructions.

The system instructions and configured preferences precede changing history and
input metadata, keeping their prefix stable for providers with prompt caching.
Caching depends on the provider and matching prefixes; compaction, edits and
configuration changes can reduce reuse. Context is still sent on every request.
Existing configuration files keep their explicit values. To adopt these defaults:

```text
@helper /config set model.contextTokens "32768"
@helper /config set model.compactTokens "24576"
@helper /config set chat.contextChars "131072"
@helper /config set chat.historyWindow "50"
```

`/metrics` reports this conversation's model attempts (including retries/tool
rounds), failures, mean latency, input/output/cached token totals, cache hit ratio,
reporting coverage, latest measured input/output counts, pre-request token budget
and its source (`provider`, `provider+estimate`, or `estimate`), history size and
rebuilds. It needs no model call and exposes
no prompts, credentials or other conversations' counters. Counters are in memory,
reset on worker/transport replacement, and capped at 64 conversations with LRU
eviction. Missing provider fields remain **unknown**, while an explicit zero is
reported as zero. Ratios use only requests with both input and cached counts.
Debug logs include the same numeric counters per model attempt.

Set `log.level` to `debug` to trace input selection (`not_addressed`,
`send_not_permitted`, `existing_task` or `own_message`), queued/running/finished
tasks, command catalogue and context preparation, model request starts, HTTP
statuses, the first streamed text/reasoning/tool delta, tool execution and final
usage. Message IDs and counts correlate stages without logging message bodies,
personality prompts, provider response bodies, tool arguments/results or credentials.
Upstream HTTP denials, structured refusals and content-filter blocks log at
`error`, including non-retryable responses, even with `log.level=info`.
If there is no `model request started` line, inspect the preceding task-selection
and preparation logs; socket notifications alone do not imply a model call.

Streaming requests ask for `stream_options.include_usage`; gateways explicitly
rejecting that optional field fall back without it. Both DeepSeek
`prompt_cache_hit_tokens` and standard `prompt_tokens_details.cached_tokens` are
recognized. Test caching with repeated identical synthetic inputs, then compare
real appended context; latency alone cannot establish a cache hit.

## Safe server command retries and API demonstrations

`BotApi.executeCommand(conversation, command, key)` requires a durable key.
The server atomically stores its effect and outcome, and repeated calls return
the same outcome for 24 hours. `BotApi.commandReceipt(key)` retrieves its status.
The worker records keys before execution and resolves pending keyed commands on
restart, then supplies the recovered tool results to the model. It stops automatic
replay after 23 hours for safety margin; legacy unkeyed pending tasks remain
blocked. Receipt expiry, credential revocation and deleted accounts still apply.

The real integration test is an executable API walkthrough: it issues/revokes a
scoped credential using `issueCredential`, sends through that credential, creates
a room and a stable DM through `createConversation`, sets/removes reactions with
`react`, and exercises native commands, discovery and model history tools. It
runs entirely against disposable data. Chat configuration is demonstrated
through owner/admin chat commands. Credential issuance
and conversation creation lack retry keys and are deliberately not retried after
an ambiguous transport failure.

Each mention triggers one agent turn. The model receives the current sender's
username and stable account ID and may use tools, reply, or finish silently with
an empty final response. A silent decision is checkpointed and is not retriggered
by notifications or reconnects. Replies use the durable outbox; model command
tools omit `/reply` and `/tell` to prevent a tool send plus a second final answer.

The default system prompt favors brief, natural conversation in the user's
language, with only the detail needed and no routine preamble or formal summary.


## Autonomous group participation

As the bot owner or a global administrator, enable it through chat:

```text
@helper /config set autonomous "true"
@helper /config set model.maxToolCalls "16"
```

`autonomous` defaults to false. Existing explicit tool limits are preserved;
the second command raises an older saved limit of four. The flag applies to all
readable, writable rooms available to this worker, without changing server grants
or DM behavior. Set it to `"false"` to return to mention-only room input.

The first new room message starts a fixed three-second collection window.
New arrivals join that window without postponing its deadline. Pending room
messages are considered together, at most once every three seconds per room;
tool calls within a turn run immediately. Each batch is bounded to 100 inputs
and approximately 16,000 UTF-16 code units. The agent sees sender names/IDs,
message IDs, mentions, replies and recent context. It is prompted to join only
when useful, combine related questions in one plain group message, and answer
independent questions with separate replies in the same decision turn. It should
write minimally and naturally, without routine headings or narrating tools.

The model can return plain final text or use `finish_turn` with a `messages`
array. Omit `reply_to` for a plain group message, supply an input ID for a targeted
reply, or return an empty array for silence. The complete plan is saved before
publishing, so interrupted multi-message delivery resumes without another model
call. A finishing tool ends the model round immediately, preventing an extra
summary. Own output is ignored; quiet decisions do not display typing activity.
Worker commands still use the owner/admin checks and do not enter model context.

`model.timeoutMs` bounds the whole model turn, including preparation, retries and
tool rounds. Upstream HTTP failures are handled before reading their response
bodies. Timeout/error paths release the active queue; an abandoned model cannot
execute late tools. Failed receipt lookups and inaccessible unresolved effects
are retained for recovery without stopping healthy conversations. The worker
checkpoint upgrades v2 to v3 atomically on its next save, preserving cursors,
outputs and command receipts; older workers reject v3 rather than replaying batches.
