# Building an agent against frozen v1

This guide describes the worker responsibilities and our recommended starting
point. [Bots](bots.md) and its [schema](bot-v1.schema.json) define the frozen wire
contract. Keep models, tools and scheduling outside CommonRoom; the server owns
identity, scoped chat access and retained history.

## Start with the reference worker

`web/scripts/bot/agent.mjs` is a mention-only worker with a fixed reply. Replace
its `onMessage` body with a model call. `client.mjs` owns HTTP/WS, catch-up and
checkpoints; `contract.mjs` validates actual payloads against the published
schema. This separation makes transport behavior reusable without an agent
framework. Node 22.18+ and the project's locked development dependencies suffice.

Provision `/bot new helper support` and `/bot token new helper worker`, then
inject the one-time secret as `BOT_TOKEN`. Never put it in code, URLs or logs.

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
separate from this processing checkpoint. Edits/reactions preserve sequence;
an agent responding to those changes needs its own explicit state and policy.

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

External tools need their own durable task IDs/retry policy. A chat receipt does
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

V1's fields, endpoints, limits and meanings are frozen. Keep implementation fixes
compatible; v1 is the sole bot API contract. The schema fingerprint and live
contract-validation tests are the guardrails for future development.
