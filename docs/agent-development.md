# Building an agent against v1

This guide describes the worker responsibilities and our recommended starting
point. [Bots](bots.md) and its [schema](bot-v1.schema.json) define the wire
contract. Keep models, tools and scheduling outside CommonRoom; the server owns
identity, scoped chat access and retained history.

## Start with the reference worker

`web/scripts/bot/agent.mjs` is a mention-only worker with a fixed reply. Replace
its `onMessage` body with a model call. `client.mjs` owns HTTP/WS, catch-up and
checkpoints; `contract.mjs` validates actual payloads against the published
schema. This separation makes transport behavior reusable without an agent
framework. Node 22.18+ and the project's locked development dependencies suffice.

Provision `/bot new helper support` and `/bot token new helper worker`, then
use the operator credential to issue a restricted expiring worker token through
`POST /api/bot/v1/credentials`, then inject that secret as `BOT_TOKEN`. Never put it in code, URLs or logs.

```sh
rtk pnpm --dir web install --frozen-lockfile
# BOT_TOKEN is injected privately; CHAT_URL includes the hosting base path.
rtk proxy env CHAT_URL=http://127.0.0.1:3000/ node web/scripts/bot/agent.mjs
```

Use HTTPS/WSS through the proxy remotely. `BOT_STATE` selects a checkpoint file;
the default is `data/bot-agent-state.json`, or under `CHAT_DATA` when set.
The worker creates its parent directory privately. The existing `/data/` ignore
rule covers the default checkpoint and temporary replacements; keep custom paths
private and out of version control. A separately deployed worker uses its own
local data folder.
One worker owns one checkpoint. Files use mode `0600` and atomic rename, covering
process interruption rather than power-loss durability. Use a transactional,
fsynced worker store if stronger durability or multiple workers are required.

## Discover and consume input

`/me` returns the account and readable conversation directory. Persist cursors
by **conversation ID**, never by room name. The reference baselines newly seen
IDs at `unread.through`, avoiding replies to old mentions. Set `replayHistory:true`
only for intentional retained-history processing. Rename preserves a cursor;
new/regranted identities use the selected baseline policy. Another account's
checkpoint is rejected.

The socket supplies snapshots and conversation notifications. The reference
coalesces repeated signals and fetches ascending history pages after saved
cursors. This handles more than 50 missing messages while connected and after
reconnect without retaining a growing frame queue. A possible retention gap
calls `onGap`; global sequence gaps alone do not prove lost input.

`onMessage(message,{view,conversation,username,client})` runs sequentially for
incoming messages. Own messages are skipped by `author_id`. Advance the cursor
only after the handler succeeds. `/read` controls shared unread state and is
separate from this processing checkpoint. **Never call `/read` to acknowledge processing**: it changes the
human-facing unread badge for the whole account. Leave `read_ack` out of worker
credentials unless shared unread changes are intentional.

Edits/reactions/retractions change `content_revision` without advancing message
sequence. `onRevision({conversation,view,previous,revision,client})` runs when a
saved revision differs, even with no new input. Stateful workers should use it to
rescan retained history with a pinned `content_revision`, compare message IDs and
content with their own durable state, and cancel/update affected tasks. It can also
fire for sends; it is not a classified change event. Commit the worker's changes
before returning; failed callbacks leave the saved revision unchanged. The default
reference handler consumes new mentions only. A null unread entry defers that
conversation without resetting its checkpoint. Checkpoint format is now version 2;
old checkpoints require deliberate reconciliation before replacement.

## Produce reliable output

Inside the handler, call `client.send(text,{replyTo,markdown,slot})`. Use a stable
slot per intended output from one input, such as `answer` or `tool-summary`.
The client saves the request/key before sending, saves its acknowledgement, then
commits the input cursor and retires that input's outbox in one checkpoint.
Recovery reuses the saved answer even if a repeated model call returns new text.
Stable IDs avoid destination-name races.

Unresolved sends stop at 23 hours, before the server's 24-hour deduplication
window expires. Inspect history and the pending request before resolving them;
automatically issuing a fresh key could duplicate output. Permission failures
with pending sends preserve the outbox for inspection. Removed input or an
inaccessible pending destination also stops for inspection. GET/read/desired-state
reaction operations have bounded retries; unavailable upstream responses back off.

Use `client.sendLong(text,options)` for long outputs; it preserves numbered part
slots and adds `[i/n]` continuation markers. Send an `activity` socket frame with
`state:"working"` while waiting on a model/tool, refresh it within 30 seconds, then
send `idle`. This ephemeral signal does not acknowledge or claim an input.

**Chat retries deduplicate delivery, not external effects.** External tools need
their own durable task IDs/retry policy. `client.taskId(slot)` returns a stable
conversation/input-message/slot identifier during `onMessage`; supply it to an
external idempotent API or durable execution ledger. It does not itself prevent
an effect. Include a worker-owned content revision in task policy when edits
should deliberately create a new operation. A chat receipt does
not make a payment, file write or other side effect exactly-once. We recommend
starting with one mention-triggered model call and one reply, then adding tool
budgets, deadlines, cancellation and bot-to-bot loop rules deliberately.

## Configuration and deployment

The reference does not register a configuration handler. Add one using the
schema's `ClientFrame`/`ServerFrame` definitions when live settings are needed.
The worker validates accepted keys, persists settings and redacts secrets; the
server authorizes/correlates requests. Ambiguous sets require fetching current
state before retrying.

Use a supervisor, private checkpoint storage and secret injection. Permanent
credential/contract errors stop the worker; inspect its state before restarting.
Run [Testing](testing.md), then check the real hostname/base path, credential
rotation, network interruption, restart and expired input. Log operational IDs
sparingly and omit message bodies, prompts and credentials.

V1 is the sole bot API namespace. Keep the published schema, worker and live
contract tests aligned when changing it; browser endpoints remain separate.
