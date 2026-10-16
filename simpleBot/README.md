# simpleBot — a Bot API v1 reference worker

A TypeScript worker for CommonRoom's Bot API v1, running on Deno 2.5+ with
native Deno KV persistence. No application dependencies, build step, or agent
framework are required. Deno KV currently needs `--unstable-kv`, enabled by the
worker tasks. The repository's Node/TypeScript tools are used for source checks.

From `simpleBot/` (or its `data/` subdirectory):

```sh
deno task config model.name "ds-zf/deepseek-flash"
deno task start
# Restart when source files change, keeping logs visible:
deno task dev
```

The default permission profile permits reads/writes only under `./data`, plus
network and environment access, and permission to launch the Deno executable for
restricted code execution. Add any external `CHAT_DATA`, `BOT_CONFIG` or
`BOT_STORAGE` paths to its read/write permissions if overriding those locations.
Permission profiles are experimental in Deno and may print a warning. Model and
chat connection settings remain in `data/config.json`, not in `deno.json`.
`--with-shell` selects the separate launch permission profile described below;
it is never a config key.

It is deliberately more than a "hello world" reply loop. One bot demonstrates
the core chat features the v1 surface offers, so the code doubles as executable
documentation:

| Feature                                                    | Where                                                |
| ---------------------------------------------------------- | ---------------------------------------------------- |
| Snapshot catch-up with per-conversation cursors            | `src/agent-input.ts` (`syncOnce`, `advance`)         |
| Keyed idempotent sends with a durable outbox               | `src/outbox.ts` (`sendParts`), `src/store.ts`        |
| Multi-part output over the 4,000-character limit           | `src/api.ts` (`chunkMessage`), `sendParts`           |
| Edit/retraction handling via `content_revision`            | `src/agent-input.ts` (`reconcile`)                   |
| Revision-pinned history rescans (`409 conflict` → restart) | `src/api.ts` (`history`)                             |
| Replies, mentions, desired-state reactions                 | native command tools and typed `BotApi.react`        |
| Transient `working`/`idle` activity signals                | `beginActivity`, `setActivity`                       |
| Owner/admin configuration through chat commands            | `src/agent-controls.ts`, `src/config.ts`             |
| Secret redaction in configuration fetches                  | `ConfigStore.fetch`                                  |
| Presence directory (account-wide credentials)              | `src/main.ts`, private status reports                |
| Shared-unread acknowledgement behind explicit opt-in       | `acknowledge_read` tool, `chat.acknowledgeReads`     |
| Credential scopes and expiring tokens                      | `/status`, `snapshot.credential`                     |
| Conversation creation (rooms and DMs)                      | native `/new room`, `/tell` and typed client methods |
| Durable KV state and model-owned conversation memory       | `src/store.ts`                                       |
| Bounded retries, `Retry-After`, typed error codes          | `src/api.ts`                                         |

## Files

```
simpleBot/
├── src/
│   ├── main.ts        entrypoint: config, transport, lifecycle, logging
│   ├── launch.ts      select worker launch permissions and forward signals
│   ├── startup.ts     immutable command-line capabilities
│   ├── shell.ts       optional shell tool and quick-result grace handling
│   ├── shell-process.ts command process-group supervision and bounded output
│   ├── agent.ts       lifecycle, transport convergence and component wiring
│   ├── agent-input.ts history convergence, source revisions and input admission
│   ├── agent-loop.ts  per-conversation model ticks and event scheduling
│   ├── agent-model.ts model context, history and memory preparation
│   ├── agent-tools.ts native tool execution and keyed command receipts
│   ├── agent-jobs.ts  durable worker-owned background jobs and shared locks
│   ├── agent-controls.ts local public/owner command handling
│   ├── agent-state.ts shared worker services and per-conversation activity
│   ├── scheduler.ts   fair bounded scheduling and interruptible deadlines
│   ├── tick.ts        decision schema, validation and durable session types
│   ├── tick-response.ts normalize native calls and tick envelopes without effects
│   ├── tick-commit.ts atomic decision/outbox/job publication
│   ├── tick-request.ts tick context and custom model adapter compatibility
│   ├── outbox.ts      durable multipart output and acknowledgement
│   ├── context.ts     append-only history prefix and on-demand pages
│   ├── tokens.ts      provider token baselines and bounded context compaction
│   ├── metrics.ts     numeric model/cache diagnostics per conversation
│   ├── api.ts         typed HTTP client, idempotency, chunking, retries
│   ├── socket.ts      WebSocket notifications, discovery/activity frames, reconnect
│   ├── wiring.ts      socket event handlers (shared by the entrypoint and tests)
│   ├── config.ts      runtime settings, validation, redaction
│   ├── store.ts       asynchronous worker state and recovery
│   ├── state.ts       persisted worker types and validation
│   ├── records.ts     targeted KV transactions and large-record chunks
│   ├── tracked.ts     nested mutation tracking for targeted writes
│   ├── kv.ts          Deno KV runtime boundary
│   ├── memory.ts      model-owned asynchronous document workspace
│   ├── execution.ts   code tool, workspace checks, and request validation
│   ├── execution-process.ts restricted Deno subprocess lifecycle
│   ├── prompts.ts     conversation and memory instructions
│   ├── tools.ts       model tool definitions
│   ├── model.ts       OpenAI-compatible chat client
│   ├── wire.ts        v1 wire types transcribed from the published schema
│   ├── log.ts         shared logging contract
│   └── config-cli.ts  inspect or seed the local config file
├── test/
│   ├── mock-server.ts in-process Bot API v1 stand-in
│   ├── agent.test.ts  worker and checkpoint regression tests
│   ├── config.test.ts JSON/CLI persistence and runtime configuration
│   ├── context.test.ts context budget and history retrieval
│   ├── execution.test.ts filesystem/network permissions and process bounds
│   ├── prompts.test.ts autonomous/mention-only/DM prompt policies
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
BOT_TOKEN=bot_... deno task --config simpleBot/deno.json start
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

| Key                        | Default                  | Notes                                                                            |
| -------------------------- | ------------------------ | -------------------------------------------------------------------------------- |
| `prompt`                   | _(empty)_                | Optional personality and user preferences; up to 4096 UTF-8 bytes                |
| `chat.url`                 | `http://127.0.0.1:3000/` | the only preset value; include the base path, omit `/api/bot/v1`                 |
| `chat.token`               | _(from `BOT_TOKEN`)_     | bot credential; redacted on fetch                                                |
| `model.endpoint`           | _(empty)_                | OpenAI-compatible base URL or a full `/chat/completions` path                    |
| `model.token`              | _(empty)_                | optional bearer key; omitted entirely when empty                                 |
| `model.name`               | _(empty)_                | model id sent to the endpoint                                                    |
| `model.timeoutMs`          | `120000`                 | 1000–600000                                                                      |
| `model.stream`             | `true`                   | streamed completions                                                             |
| `model.maxTokens`          | `4096`                   | output budget including reasoning; 128–32768                                     |
| `model.reasoningEffort`    | _(empty)_                | provider default when empty; none, low, medium or high if supported              |
| `autonomous`               | `false`                  | Participate in writable rooms without mentions, using fixed collection windows  |
| `chat.respondToAllDms`     | `true`                   | answer every direct message, not only mentions                                   |
| `chat.acknowledgeReads`    | `false`                  | enables the read acknowledgement tool                                            |
| `chat.historyWindow`       | `50`                     | prior messages requested for model context, within the character budget          |
| `chat.contextChars`        | `131072`                 | Additional history character ceiling; 0–524288, 0 disables history               |
| `chat.batchWindowMs`       | `1000`                   | Autonomous room collection window; integer 0–10000 ms; mentions and DMs are immediate |
| `model.contextTokens`      | `32768`                  | Total input/output token budget; measurements plus estimates for new content     |
| `model.compactTokens`      | `24576`                  | Input threshold for context compaction; less than total                          |
| `model.maxToolCalls`       | `32`                     | Maximum tool jobs per work episode; 1–128                                        |
| `agent.maxConcurrentTicks` | `4`                      | Maximum simultaneous model requests across conversations; 1–128                  |
| `agent.maxConcurrentJobs`  | `8`                      | Maximum active tool jobs; 1–128                                                  |
| `agent.maxTicks`           | `32`                     | Model requests per work episode, including retries; 1–128                        |
| `agent.inboxLimit`         | `1000`                   | Maximum queued/running accepted inputs; 10–5000; scanning pauses when full       |
| `agent.maxEpisodeTokens`   | `262144`                 | Episode input/output token budget; 1024–16777216; estimates when usage is absent |
| `agent.episodeTimeoutMs`   | `600000`                 | Episode deadline; 1000–86400000 ms                                               |
| `chat.stateLimit`          | `200`                    | 10–5000 retained task records                                                    |
| `log.level`                | `info`                   | `debug`, `info`, `warn`, `error`, `silent`                                       |

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
catalogue and a structured tick decision containing native tool actions. For example, `@helper create a room named
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
deno test --unstable-kv -A --no-check simpleBot/test/*.test.ts
# After building the Rust server: real HTTP/WS and entrypoint lifecycle
BOT_INTEGRATION=1 deno test --unstable-kv -A --no-check simpleBot/test/integration.test.ts
deno task --config simpleBot/deno.json start  # run it
pnpm --dir simpleBot check           # typecheck with the project dev tools
```

The unit tests use an in-process loopback HTTP server and a stub model, with no external server or credentials. The opt-in integration test launches an isolated Rust server, the actual worker process, and a keyless local model gateway under a hosting base path. They cover first-run baselining, idempotent replay, additive edit
passes, an edit arriving mid-answer, retraction cancellation, local commands,
unknown-command passthrough, chunking, reactions, the `/read` opt-in, credential
scope failures, configuration round-trips and rejection, checkpoint durability
and version refusal, cross-account refusal, activity signals, and the
`unread: null` deferral contract.

## Operational notes

- **Durable storage**: `simpleBot/data/agent.kv`, mode 0600, opened asynchronously.
  `BOT_STORAGE` overrides the path; `CHAT_DATA` moves the default directory.
  Deno KV owns the SQLite database and its sidecar files. One worker owns one
  database, bound to its bot account; stale concurrent writers fail instead of
  overwriting another worker. Stop the worker before backing up the whole data directory.
- **Fresh KV format**: old `checkpoint.json` and `BOT_STATE` are ignored, with no
  import or migration. The first run baselines from the server's current state;
  subsequent restarts recover cursors, message versions, tasks, group decisions,
  command intents/results and output keys from KV. Output is durable before send;
  scan cursors advance after durable input admission, independently of delivery.
  Validated worker KV v1 records migrate to v2 without changing pending keys.
- **Targeted commits**: only changed records are encoded and written. Nested
  task mutations are tracked; small records publish directly in an atomic KV
  transaction. Large plans use immutable chunks and atomic record heads, so an
  interrupted staging write never publishes a partial response plan. Startup
  reclaims unreachable chunks older than a day. Storage failures stop later
  checkpoint commits rather than skipping a durability boundary.
- **Operator configuration** remains in `config.json`, with the existing CLI,
  reload, validation, and secret redaction. It is outside the model's workspace.
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

`Agent` wires input convergence, the event scheduler, local controls and output.
Extend `agent-loop.ts` for scheduling, `agent-tools.ts` for native effects, or
`agent-controls.ts` for local commands. Keep the persistence rules intact:

1. Persist the output request (payload and idempotency key) **before** sending.
2. Commit accepted input with its scan cursor; commit tick consumption, actions
   and output together before effects. Delivery acknowledgements remain separate.
3. Give each intended output a stable slot name so recovery reuses it.
4. Treat `content_revision` as "something in retained history changed" and rescan
   with the revision pinned; do not assume it means an edit.

The server-side contract is in [`docs/bots.md`](../docs/bots.md), the normative
schema in [`docs/bot-v1.schema.json`](../docs/bot-v1.schema.json), and worker
guidance in [`docs/agent-development.md`](../docs/agent-development.md).

Native commands use separate durable server receipts. The worker saves command
intent/key/time before execution and reuses the identical effect key during
recovery within the receipt window. Distinct proposed actions in separate ticks
have distinct keys, even if their arguments match. On restart it recovers pending
keyed outcomes before asking the model to continue. Legacy unkeyed or expired
uncertain actions require inspection. The KV store retains these records across restarts. Run one worker per database/account.

Model history uses a bounded `read_history` tool that filters worker control messages and their responses, matching the normal context filter. The bounded tool remains the budgeted path for history; the server's `/history` command is offered like any other grant.

## Bounded history and low-token operation

Public `@helper /help`, `@helper /ping` and `@helper /metrics` are available to ordinary room members
and use no model calls. `/admin` lists the separate owner/admin controls; other
members do not receive that private catalogue.

History context stays in memory, in chronological chunks that grow by appending.
The initial baseline requests 50 recent messages; later answers append intervening
messages instead of sliding the prefix on every answer. Before reuse, the worker
revalidates cached messages against retained server history, rebuilding on edits,
removal or retention. Raw provider conversations are not persisted. A bounded
trace of accepted tick decisions and durable job outcomes survives restart;
retained chat history is fetched again for the next tick.
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
The command renders Markdown tables with latest/mean/P95 latency and sample
counts for end-to-end replies, queueing (including collection), source validation,
context preparation, model attempts, first streamed deltas, decision commits,
delivery queueing and delivery. End-to-end timing starts when the worker admits
an input and ends after its complete response's final part is acknowledged and
saved. It includes tool waits and retries. Group members receive individual
admission-to-delivery samples even when they share one response. Local controls,
silent decisions and missing admission times after restart/metrics eviction do
not create end-to-end samples. Replaced/retracted versions cannot complete an
older version's timer. Mean model calls per answered input counts shared batch
calls for each input up to its final plan commit; later background calls do not
increase that answer's step count. It measures response steps rather than billing.
P95 uses the latest 128 samples per stage; means and counts cover the current
metrics lifetime. Stage rows have different denominators and must not be added.
Counters also show source inputs validated versus HTTP reads and the configured
collection window. Nearby inputs share bounded sequence-range reads; sparse
ranges use at most four concurrent reads, including control detection. Commands
are re-read immediately before execution. Byte-limited pages pin their revision;
a conflict restarts the whole range once without mixing old and new rows.
Preparation latency is measured separately from inference. The report also
includes time to the first streamed text, reasoning or tool delta, with reporting
coverage; buffered requests do not invent a first-delta measurement. Independent
catalogue, retained-history and workspace reads overlap during preparation. The
command catalogue is still fetched for every tick so changed grants apply live.

Set `log.level` to `debug` to trace input selection (`not_addressed`,
`send_not_permitted`, `existing_task` or `own_message`), queued/running/finished
tasks, command catalogue and context preparation, model request starts, HTTP
statuses, the first streamed text/reasoning/tool delta, tool execution and final
usage. Message IDs and counts correlate stages without logging message bodies,
personality prompts, provider response bodies, tool arguments/results or credentials.
Upstream HTTP denials, structured refusals and content-filter blocks log at
`error`, including non-retryable responses, even with `log.level=info`.
Tick diagnostics include `model tick request`, `model tick response` and an
explicit `model tick accepted` or `model tick rejected`, correlated by
conversation and tick ID. Response summaries contain tool names from the host's
catalogue (unknown names are labelled `unavailable`), call counts, argument
lengths/JSON types and decision shape without the values. Rejections distinguish
unexpected calls, invalid JSON and invalid decisions. `tick retry scheduled`
records the reason, retry count and delay; `tick recovery scheduled` records the
single tool-less recovery tick, and `input unavailable` records an admitted input
whose message is gone from retained history. `episode tool budget reached`
records dropped actions and the published messages. A model-attempt
`success=true` means
inference returned successfully; only an accepted and committed decision starts
effects. If there is no `model tick request` (or legacy `model request started`)
line, inspect task-selection and preparation logs; socket notifications alone
do not imply a model call.

Streaming requests ask for `stream_options.include_usage`; gateways explicitly
rejecting that optional field fall back without it. Both DeepSeek
`prompt_cache_hit_tokens` and standard `prompt_tokens_details.cached_tokens` are
recognized. Test caching with repeated identical synthetic inputs, then compare
real appended context; latency alone cannot establish a cache hit.

Tool schemas stay identical when only reply IDs or per-tick reply limits change.
Those constraints travel in Tick state; the host checks targets and the final
message count before committing any jobs or output. Current command grants and
repair feedback follow retained history, leaving the instruction/history prefix
stable when those fields change.

An opt-in synthetic check on 2026-10-16 verified the Tailscale Aperture route
`https://ai.snowy-richter.ts.net/v1` and its advertised `deepseek-flash`
(DeepSeek V4.1 Flash) model without a client API key. The spelling is **richter**.
The existing `reasoning_effort: "none"` setting returned a non-thinking reply;
streaming/cache usage continues to use the existing compatible request fields.
No OpenAI-specific cache retention, cache key or paid service-tier fields are
enabled automatically. DeepSeek's upstream cache is automatic.

| Synthetic follow-up | Cached input | Total input | Cache ratio |
| --- | ---: | ---: | ---: |
| Changing reply-ID enum | 256 | 3726 | 6.9% |
| Stable schema, first follow-up | 3456 | 3718 | 93.0% |
| Stable schema, second follow-up | 3456 | 3718 | 93.0% |

These are provider counters from five bounded synthetic requests, including
initial requests to populate each prefix. They establish cache reuse for that
route at the time of the check; the small sample does not establish a latency
improvement or production-wide hit rate. No private chat content was sent.

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

Each eligible message can start a model tick; the model can request further ticks without another message. The model receives the current sender's
username and stable account ID and may use tools, reply, or finish silently with
an empty `finish_tick.messages` array. A silent decision is checkpointed and is not retriggered
by notifications or reconnects. Replies use the durable outbox. The fetched server
catalogue is the worker's whole command ACL: every advertised command, including
`/reply` and `/tell` when granted, is offered to the model, and an unadvertised or
unauthorized command is refused by the server on execution. The prompt still asks
for one path per answer, because a granted send command and a final answer are two
separate messages.

The system prompt gives the model freedom to choose its response and manage its
workspace. It distinguishes speaker claims, proposals, agreements and verified
actions, while keeping server authorization in code.

## Autonomous group participation

As the bot owner or a global administrator, enable it through chat:

```text
@helper /config set autonomous "true"
@helper /config set model.maxToolCalls "32"
```

`autonomous` defaults to false. Existing explicit tool limits are preserved;
the second command gives an older configuration more room for memory and tool use. The flag applies to all
readable, writable rooms available to this worker, without changing server grants
or DM behavior. Set it to `"false"` to return to mention-only room input.

The first new room message starts a fixed collection window (one second by default).
New arrivals join that window without postponing its deadline. Unmentioned pending room
messages are considered together after the collection window. Direct mentions
can accelerate it; tool completions and model-requested continuations run when ready. Each batch is bounded to 100 inputs
and approximately 16,000 UTF-16 code units. The agent sees sender names/IDs,
message IDs, mentions, replies and recent context. It is prompted to join only
when useful, combine related questions in one plain group message, and answer
independent questions with separate replies in the same decision turn. It should
write minimally and naturally, without routine headings or narrating tools.

Tune the delay live with `@helper /config set chat.batchWindowMs "250"` for a
short window, `"0"` for no collection delay, or `"3000"` for the older behavior.
The allowed range is 0–10000 ms. Existing admitted windows keep their saved
deadline when the setting changes; new windows use the new value. Direct
mentions and DMs remain immediate. Legacy v1 group checkpoints retain their
original three-second recovery deadline.

The model uses `finish_tick` with a `messages` array and an explicit `next`
continuation. Omit `replyTo` for a plain group message, supply an input ID for a
targeted reply, or return an empty array for silence. The complete plan is saved before
publishing, so interrupted multi-message delivery resumes without another model
call. Each finish_tick decision completes one model request; only its continuation
or eligible new input schedules another request. Own output is ignored; quiet decisions do not display typing activity.
Worker commands still use the owner/admin checks and do not enter model context.

`finish_tick` is the only advertised tool. Every other tool is described inside
`finish_tick.actions`, including its operational description, so a tick proposes
all of its independent work at once instead of spending one request per tool.
A native call that arrives anyway is
still accepted and started as a durable job. Unknown argument fields are dropped
with a host-owned label rather than rejecting the plan, and a missing or shorthand
`next` becomes `wait`; the host logs the deviation. Arguments that are fenced,
prose-wrapped or double-encoded JSON are repaired before parsing is declared
failed. Unavailable tools, malformed types, duplicate action IDs and host checks
still reject the whole plan before any effect starts.

`model.timeoutMs` bounds preparation and one inference attempt; tool jobs have
their own deadline, also bounded by the work episode deadline. A rejected request
logs its status, body size and allowlisted `error.type`/`code`/`param` labels,
omitting unknown labels and replacing parameter array indices with `[]`,
never the provider's body text, and it is read under a short deadline so a stalled
gateway cannot stall the worker. Timeout/error paths release the active queue; an
abandoned model cannot
execute late tools. Failed receipt lookups and inaccessible unresolved effects
are retained for recovery without stopping healthy conversations. The worker persists group membership, response plans, timing and silent decisions
in KV; obsolete JSON checkpoints are not loaded.

## Async event loop and model ticks

A **tick is one model request**. The production worker uses `ModelClient.tick()`;
there is no hidden inference/tool loop inside that request. Provider retries and
validation repairs are later ticks with their own IDs and budget charges.

The loop is `await relevant event → tick → persist decision → await next event`.
A conversation has at most one active model request. A fair FIFO scheduler runs
multiple conversations concurrently, bounded by `agent.maxConcurrentTicks`.
Local worker commands bypass inference. Slow tools, future deadlines and network
delivery do not occupy model slots.

The request uses the OpenAI-compatible chat-completions `tools` field and leaves
tool selection at its default. It does not force a named `tool_choice`, which
some thinking models reject. The operator's reasoning setting remains unchanged.
An ordinary assistant final-text response ends the tick with that answer and
`next=wait`; empty final text means silence. Only `finish_tick` is advertised: a
native-only response is still accepted and starts durable jobs, and a response may
mix native calls with one `finish_tick`; an
action represented both ways is coalesced after argument normalization so it
executes once. Multiple
`finish_tick` calls or unavailable tools reject the entire
plan before any effects are accepted. Text accompanying tool calls is not a
final reply; use `finish_tick.messages` to commit a response alongside jobs.

The preferred response is one `finish_tick` function call:

```json
{
  "messages": [],
  "actions": [
    {
      "id": "lookup",
      "tool": "memory",
      "arguments": { "action": "get", "key": "index" }
    }
  ],
  "next": { "kind": "on_jobs", "jobs": ["lookup"], "mode": "all" }
}
```

`messages` may be empty for silence, contain one combined response, or contain
several messages with optional `replyTo` input IDs. This works identically in
rooms and DMs. `actions` proposes independent jobs using the advertised native
tool schemas. The host validates the whole decision and saves messages, job
intents and continuation together before dispatch. Provisional stream deltas
never publish effects. An action's success is available in a later tick; do not
claim success merely because a job was proposed.
Omitted `messages` or `actions` lists default to `[]`; explicit invalid values
still fail, and `next` is always required.
Action IDs are unique strings of 1–64 Unicode characters within a tick. Validation
repairs receive the failing field and constraint without exposing argument values
in logs. Every request includes the current non-empty configured `prompt`,
including repair and job-continuation ticks.
Unknown-argument feedback identifies the tool and its allowed fields. After two
unsuccessful repair attempts, one final recovery tick omits all tools and asks
for ordinary final text using known results, or a brief acknowledgement that the
requested work could not be completed. It cannot start jobs or schedule another
tick, so it cannot repeat an effect. The same single recovery tick is spent when
the host would otherwise end an episode with an admitted input still unanswered,
including a terminal provider failure and a tool-budget stop. All attempts still
share the episode budgets. New input remains queued while a repair owns its
original inputs; it starts with a fresh retry allowance after that episode
commits or fails.

Each input version owns at most one final response plan, including all combined
or targeted messages in that plan. A durable `replyHash` prevents a continuation,
job completion, rescan or restart from answering that version again, even with
different text. Send final messages after prerequisite jobs finish. New inputs
and edited versions may receive their own replies; a group continuation may
answer another input that has not received a response. A final response with no
remaining jobs does not schedule an immediate continuation. Tick state lists
valid `replyTargets` message IDs and whether each input already has a reply;
the generated reply schema uses those IDs and bounds the number of replies.
Messages assigned to one input within a decision are combined into one response.
Multiple plain group messages in a batch are also combined. If a plan mixes a
plain batch response with targeted replies, their text forms one plain response
to prevent overlapping replies; targeted-only plans can still reply separately
to independent inputs.
Continuations retain all admitted inputs, including new requirements received
while a job was running, and label them as context rather than fresh requests.
An already-observed terminal result cannot wake `on_jobs` again. A watch containing
only consumed results becomes `wait` once all inputs have replies; otherwise it
returns a repair error so an unanswered request is not silently abandoned.
DEBUG logs include job start/completion,
duration, result size, wake reasons and watched job states, without message or
tool-argument content. Reply commit/delivery logs associate source inputs, reply
targets, outbox slots and server message IDs, distinguishing repeated delivery
from replies to different inputs. INFO
logs include tick start/commit, input IDs, output count and next wake, so one tick
with multiple proposed messages can be distinguished from multiple model calls.

`next` chooses a one-shot wake:

- `wait`: await new eligible input; unwatched jobs may finish silently.
- `continue`: enqueue another tick fairly for useful independent work.
- `after` with `delayMs`: schedule one later tick, within 1 ms–24 hours and the
  episode deadline.
- `on_jobs` with `jobs` and `mode` (`any`/`all`): suspend until selected jobs are
  terminal. Reference action IDs from this decision or existing durable job IDs.

Every later tick chooses again. New eligible input can wake a conversation while
it waits for a job or deadline. It consumes the old continuation and receives
pending/completed jobs, new input and recent decisions. Old timers and unselected
completions cannot create extra ticks. Messages arriving during inference remain
queued. Own output, duplicate notifications, unchanged snapshots and presence
updates do not cause inference.

Autonomous unmentioned room messages use the configured fixed collection window;
mentions can accelerate it. DMs and direct mentions are ready immediately.
Job completions and self continuations do not incur the room batching delay.
Simple questions should finish as ordinary final text in one step. Independent
actions should share one decision and, when all results are prerequisites, one
`on_jobs`/`all` wait. Additional steps remain available for dependent work; the
worker does not add a separate planning-only or polling call. Prompts favor short
answers and omit empty decision lists while preserving complete code and detail
when needed. The output token ceiling remains unchanged to avoid truncation.

Jobs belong to the worker rather than a single tick. They have durable IDs,
intents and results and are separately bounded by `agent.maxConcurrentJobs`.
Resource locks permit concurrent conversation-memory reads and exclude writers;
history retrieval, shared filesystem scripts and account-wide server mutations
retain their appropriate exclusive locks. Model-client configuration replacement
does not replace the worker's lock manager. Tools and results remain untrusted
context data. Ticks receive bounded result excerpts; `read_job` pages a terminal
result within the current conversation when more detail is needed. Scripts can use `Promise.all` for independent network requests.

The transport cursor advances after durable input admission, independently of
model consumption and output delivery. Committed decisions and keyed output
resume after restart without another model call. Interrupted jobs retain an
interrupted/uncertain result; arbitrary scripts and external effects are never
automatically repeated. Native keyed command receipts can recover committed
outcomes. Cancellation/retraction aborts dependent work and drops unsent output;
already issued effects may have occurred and are not rolled back.

Self continuations, repairs and retries share the work episode's tick, token,
tool and wall-clock budgets. The tool budget is a host limit, not a model
mistake: actions that no longer fit are dropped together with any continuation
that referenced them, the decision's own messages are still published, and the
episode stops with a warning. An episode never ends on a host stop or a terminal
failure while one of its admitted inputs still has no reply: the tool-less
recovery tick above answers it in prose, or acknowledges that it could not.
Exceeding a budget stops automatic continuation and
records a diagnostic visible through `/status`. Provider usage is charged when
available; otherwise input estimates and the configured output allowance are
charged conservatively. A new externally admitted episode starts a fresh budget;
job deadlines already established are not extended. A worker bounds retained
job records at 128 and preserves uncertain outcomes for inspection.

See [the loop design](LOOP_DESIGN.md), `agent-loop.ts`, `scheduler.ts`,
`tick-commit.ts` and `agent-jobs.ts` for the scheduling and persistence contract.
The standalone `ModelClient.complete()` helper remains for existing custom
adapters and transport tests; the production agent uses the single-call tick API.

## Model-owned persistent workspace

Every model turn has the asynchronous `memory` tool, including when server tools
or history are unavailable. Its `get`, `set`, `list` and `delete` actions accept
arbitrary JSON documents. The model chooses its own organization: topic notes,
member preferences, summaries, plans, or unfinished work. There is no separate
extraction model, fixed fact schema, or forced public acknowledgement.

The `index` document (up to 6,000 UTF-8 bytes of content) and a bounded
list of document names are loaded each turn. Other documents (up to 30,000 bytes)
are fetched on demand. `list` supports prefix filtering and continuation cursors.
Large notes should be split into linked documents. For example:

```json
{
  "action": "set",
  "key": "topics/runtime",
  "value": { "proposal": "Deno", "agreement": "unresolved" }
}
```

`get` returns the document's version. Supply that version with `set` or `delete`
for a conditional update; `version:null` creates only if absent. Omit the version
for an intentional overwrite. Conflicts and invalid arguments return tool errors
so the model can correct its request. Successful writes return a committed version.

Workspaces are bound in code to the bot account and current stable conversation
ID. A room shares one workspace among its speakers; DMs and other rooms have
separate workspaces. The model cannot select another conversation or access
worker records, outbox, configuration, or credentials through this tool. Stored
documents include the current input IDs, stable author IDs and content hashes as
provenance. History rows also include stable author IDs. Memory is contextual
data rather than instructions or authority.

A tick can propose memory updates and choose silence. Use `on_jobs` to inspect
their results in a later tick, or `wait` to let those jobs complete silently. Durable workspace documents
survive task pruning and server history retention. They are not automatically
removed when their source is edited or retracted; the model must revise or delete
stale information, or an operator must remove it. Losing room access makes the
workspace unavailable to model turns; it remains on disk if access is restored.
`chat.stateLimit` bounds delivery records, not model documents. The model is
responsible for organizing and deleting obsolete notes.

## Optional shell and binary execution

Enable shell access when starting the worker:

```sh
# From simpleBot/:
deno task start --with-shell
deno task dev --with-shell
# From the repository root:
deno task --config simpleBot/deno.json start --with-shell
# Direct invocation (supply the Deno launch grant yourself):
deno run --unstable-kv -P=with-shell src/main.ts --with-shell
```

The task launcher selects Deno permissions before starting the worker. Without
`--with-shell`, the default profile permits launching the restricted Deno code
runner and the model has no shell tool. With it, Deno grants binary execution
and the worker advertises `run_shell`. The executor independently checks the
startup capability. `config.json`, `/config`, environment settings and reloads
cannot enable or disable it; restart with different arguments to change it.
Transport/configuration rebuilds retain the original startup choice. `/status`
and the startup banner report shell availability.

```json
{
  "command": "cd ~ && ls -la",
  "waitMs": 1000,
  "timeoutMs": 60000
}
```

Each command uses a fresh `/bin/sh -c`, starting in the agent workspace. An
optional `cwd` may be absolute, workspace-relative (including `..`), `~`, or
`~/...`. Pipes, redirects, `cd`, and available binaries work normally. The shell
inherits the worker environment, including `PATH` and `HOME`. Files persist;
shell variables and directory changes do not carry to another command. Input
is closed and there is no interactive terminal session.

Filesystem, network and environment access are provided by the deployment,
such as gVisor. SimpleBot does not configure that sandbox or impose shell
command/path allowlists. The workspace is the default directory, not a shell
filesystem boundary. The existing `execute_code` tool retains its own Deno
permissions and is independent of this capability.

`waitMs` is the quick-result grace period (default 1000, range 0–30000 ms).
A quick command completes directly. If it is still running at the deadline,
the job checkpoints `waitExpired: true` and continues in the background.
Dispatch and grace waiting never hold a model slot; `next: on_jobs` wakes a
later tick when the command terminates. Grace expiry does not itself call the
model or terminate the command. Do not repeat a command to check its progress.

`timeoutMs` bounds execution (default 60000, range 100–600000 ms), independently
of the inference timeout and within the remaining episode deadline. Results
contain `ok`, `exitCode`, `stdout`, `stderr`, elapsed time, and any stop reason.
Combined stdout/stderr is capped at 32 KiB. Shell jobs share filesystem exclusion
with code execution. Timeout, output overflow, cancellation and shutdown kill
the command's process group and reap the shell. Deliberately detached processes
require external supervisor/sandbox lifecycle controls.

Shell intents/results use the durable job store. Restart reports interrupted
commands as uncertain and never automatically repeats them, including when the
replacement worker starts without `--with-shell`.

## Shared filesystem and JS/TS execution

The model has an asynchronous `execute_code` tool in autonomous rooms,
mention-only rooms, and DMs. `simpleBot/data/workspace/` is its persistent,
read/write working directory. `CHAT_DATA` relocates it along with the other data
files; changing `BOT_STORAGE` alone does not move it. The whole workspace is
shared across every room and DM served by this worker. KV memory retains its
conversation boundaries. Only intentionally shared information belongs in files.
Operator configuration, credentials and the worker KV database stay outside the
execution folder and are not provided to the subprocess.

Supply exactly one of `code` or `file`:

```json
{
  "code": "const response = await fetch('https://docs.deno.com/'); console.log(response.status); await response.body?.cancel();"
}
```

```json
{
  "code": "await Deno.mkdir('scripts', {recursive:true}); await Deno.writeTextFile('scripts/answer.ts', 'const n: number = 42; console.log(n);');"
}
```

```json
{ "file": "scripts/answer.ts", "timeoutMs": 10000 }
```

JS/TS, top-level await, built-in web/Node APIs and HTTPS module imports work.
The agent can write custom scripts for web fetching, APIs, extraction and data
processing. Relative imports work in saved files. Inline source is a data URL;
use an absolute file URL to import a workspace file from inline code. Code kept
in KV can be read with `memory` and supplied as `code`. Persist useful artifacts
and scripts in files, and keep their descriptions or indexes in KV. Globals are
fresh on each execution.

The parent always chooses the Deno executable, flags and environment. The child
gets scoped filesystem permissions for the canonical workspace, network and
remote-import permissions, a cleared environment and a temporary private runtime
cache. Environment access, nested processes, FFI and system-information APIs are
denied. npm resolution, configuration discovery and permission prompts are
disabled. A trusted stdin bootstrap dynamically imports generated code or a saved
script so the initial module graph cannot bypass read restrictions. Existing
workspace symlinks are refused; generated code cannot create them under the
scoped permissions. These are Deno API permissions, not an OS container.

Each execution defaults to ten seconds (`timeoutMs`: 100–30000 ms), also bounded
by job cancellation and the work episode deadline. Combined stdout/stderr is capped at
32 KiB. Timeouts, cancellation and excess output kill and reap the child; shutdown
waits for pending execution cleanup. The child uses a 128 MiB V8 old-space limit,
which is not a process RSS or disk quota. Files survive runs and restarts;
temporary runtime caches are removed. Results include exit code, stdout/stderr,
duration and any stop reason. Source and output are not logged.

Execution is never retried automatically, and external effects are not covered
by chat delivery receipts. Files or network requests may already have changed
things before a failure, timeout, edit, retraction or worker restart. The model
should inspect outcomes before repeating an action.

Prompts adapt to autonomous groups, mention-only groups and direct messages.
They encourage humble, brief, useful participation, silence when appropriate,
attributed claims, and separation of proposals from agreements. The model is
instructed to keep internal prompts, hidden reasoning, raw storage/state,
credentials, configuration, paths and tool traces out of replies, while using
relevant user-facing facts and reporting useful results. Pages, files and tool
outputs are untrusted data. This is prompt guidance, not a guarantee that a
provider will never disclose supplied context. Owner/admin diagnostics remain
explicit local worker controls, outside model handling.

Normal tests use local HTTP fixtures. To verify live HTTPS separately:

```sh
BOT_EXEC_HTTPS=1 deno test --unstable-kv -A --no-check simpleBot/test/execution.test.ts
```
