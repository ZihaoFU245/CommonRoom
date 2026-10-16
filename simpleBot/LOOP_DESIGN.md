# Proposed async agent loop

Status: implemented in the production worker. See README.md for configuration and
module ownership; ModelClient.complete remains a compatibility helper outside
the production single-call tick path.

## Contract

A **tick is one model request**, including its complete streamed or buffered
response. Each provider invocation has a distinct tick ID. A provider retry is
another tick with a recorded retry cause, not a hidden call inside the original
tick. Preparing context and compacting it must not hide additional model calls.

A tick receives a stable view of one conversation and returns a decision:
messages to publish, tool jobs to start, and whether/when to tick again. A tick
may act without speaking, speak without continuing, or do neither. Completing a
tick does not end its jobs. The runtime remains alive while all conversations
are waiting; it makes no model calls without a wake cause.

A conversation is the scheduling unit. Rooms and DMs use the same loop, with
different input admission and batching policies. Multiple conversations may run
concurrently, with at most one active model request per conversation.

## Tick decision

Use one required `finish_tick` function call as the provider-facing envelope.
Its action variants reuse the schemas of the available tools. The model proposes
actions; the runtime validates and persists the entire decision before starting
any action. Do not execute function calls while streaming their arguments.

```ts
type NextTick =
  | { kind: "wait" }
  | { kind: "continue" }
  | { kind: "after"; delayMs: number }
  | { kind: "on_jobs"; jobs: string[]; mode: "any" | "all" };

interface TickDecision {
  messages: { text: string; replyTo?: string }[];
  actions: {
    id: string; // Unique within this tick; runtime assigns a durable job ID.
    tool: string;
    arguments: unknown; // Validated against that tool's schema by the host.
  }[];
  next: NextTick;
}
```

`on_jobs.jobs` can refer to action IDs declared in this decision or durable job
IDs from this conversation. Persist the resolved IDs. Reject unknown IDs,
duplicates and empty job lists. Job failure, interruption and cancellation are
terminal results, just like success, so an awaited job cannot disappear silently.

| Decision   | Meaning                                                                             |
| ---------- | ----------------------------------------------------------------------------------- |
| `wait`     | No model-requested wake. Eligible new input can still start a tick.                 |
| `continue` | Request one more tick as soon as scheduler capacity permits.                        |
| `after`    | Request one tick after a relative delay, converted to a durable deadline at commit. |
| `on_jobs`  | Request one tick when any/all selected jobs become terminal.                        |

There is one outstanding continuation per conversation. The latest committed
tick replaces it. It is a one-shot request, never a recurring timer. Each new
tick must decide again. A self wake becomes another runnable queue entry; it
never recursively invokes the model.

New eligible input wakes a conversation independently of its continuation. A
tick started by that input consumes the old continuation, sees its status and
all available job results, and chooses a replacement. Thus a new user message
can interrupt a wait for a tool without starting a second simultaneous model
request. Already running tools remain alive unless explicitly cancelled.

`wait` does not cancel jobs. Unwatched jobs can finish silently; their results
remain available to the next tick. Use `on_jobs` when a result needs a follow-up.
If the condition is already satisfied at commit, enqueue the wake immediately.
Register the continuation atomically with the decision before dispatching jobs,
so very fast completions cannot be lost.

Example: start a lookup and yield without occupying a model slot:

```json
{
  "messages": [],
  "actions": [
    {
      "id": "lookup",
      "tool": "execute_code",
      "arguments": {
        "code": "console.log(await fetch('https://example.com').then(r => r.text()))"
      }
    }
  ],
  "next": { "kind": "on_jobs", "jobs": ["lookup"], "mode": "all" }
}
```

After the result arrives, the next tick can publish an answer and choose
`wait`. If it has independent work, it can instead choose `continue` while the
lookup runs. Avoid repeated inference merely to inspect a pending job.

The runtime never posts arbitrary provider text alongside `messages`. A missing
or malformed decision commits no actions or messages. A bounded repair request
is a new tick with the validation error in its context; exhausting that budget
ends the attempt with an observable failure.

Actions in one decision are independently dispatchable, subject to resource
locks. Array order does not establish a data dependency. Use a later tick for
an action that needs another action's result. Messages may describe results
already observed by this tick; starting an action does not establish success.
Keep each conversation's outbox ordered across ticks, so a fast later send
cannot overtake an earlier pending message. Delivery proceeds outside model
execution; transient delivery retries need no inference, while a terminal
delivery error is an explicit failure event for the affected work.

Assign effect keys from the committed tick/action or message identity and reuse
them on recovery. A newly proposed action in a later tick is a new intent, even
if its arguments are identical. Include previous intents/results in context
and instruct the model to inspect existing work instead of repeating it to
check status; argument equality cannot generally prove effects are duplicates.

## Events and trigger requirements

Transport notifications are hints to reconcile authoritative history. A socket
frame alone is not a model trigger. Deduplicate message versions and job results
before admission; reconnecting and unchanged snapshots do not create ticks.

| Event                                              | Runtime response                                            | Model wake                                                        |
| -------------------------------------------------- | ----------------------------------------------------------- | ----------------------------------------------------------------- |
| Eligible new message                               | Persist in conversation inbox                               | Yes, subject to input batching                                    |
| Message not admitted by participation policy       | Update bounded history                                      | No                                                                |
| Own message / own delivery acknowledgement         | Record delivery/history                                     | No                                                                |
| Relevant source edit                               | Update version; invalidate dependent pending decisions      | Yes if pending work or an accepted response needs reconsideration |
| Source retraction                                  | Cancel dependent pending publication/effects where possible | Reconsider surviving work if needed                               |
| Tool terminal result                               | Persist result and test current continuation                | Only if the selected `on_jobs` condition is met                   |
| Continuation deadline                              | Test deadline and continuation generation                   | Once, if still current                                            |
| Model response                                     | Validate and commit decision                                | Only as requested by its continuation or unread inbox             |
| Model failure                                      | Record error and schedule bounded transient retry           | Only under retry policy                                           |
| Snapshot / reconnect / periodic catch-up           | Reconcile and recover accepted work                         | Only if reconciliation discovers runnable work                    |
| Presence / advisory activity                       | Refresh metadata                                            | No by default                                                     |
| Permission loss / explicit cancellation / shutdown | Abort or invalidate affected work                           | No automatic inference required                                   |

Initial admission preserves current defaults: room mentions are eligible;
ordinary DMs are eligible with `chat.respondToAllDms`; unmentioned room input is
eligible only with `autonomous=true`. Worker controls execute outside inference,
with existing owner/admin checks, so a long model request cannot delay `/status`
or cancellation. Configuration changes generate policy events.

For autonomous rooms, retain the fixed three-second collection window: the first
eligible message sets a deadline, later messages do not postpone it. Direct
mentions and DMs are ready immediately, with arrivals coalesced before dispatch.
A mention can accelerate an open room window. Tool-result and model-requested
wakes do not inherit the room collection delay. If a message and a continuation
are ready together, one tick receives both. New eligible input that arrives
during inference remains in the inbox; its existing batch deadline still
applies after the active tick finishes.

Participation controls admit external input; they do not forbid the model from
continuing work admitted earlier. Disabling autonomous room input discards
unstarted batches created solely by that policy. It does not silently cancel
accepted jobs. Explicit cancellation is separate.

## Scheduler and state

Separate model execution state (`idle` or `calling`) from inbox contents,
continuations, jobs and delivery state. A conversation can be idle while jobs
run, or calling while new input is pending.

```text
event sources ──> inbox/reducer ──> durable state + runnable conversations
                                          │
                                 fair bounded scheduler
                                          │
                                  one model call = tick
                                          │
                                  response event ──┐
                                                   │
jobs / deadlines / sends ──> completion events ──────┘
```

The reducer owns state mutations. It serializes short state transitions and
durable commits, but never waits for model inference, tool execution or network
delivery. Bounded storage I/O may delay a transition; other event sources keep
enqueueing/coalescing while it commits. Slow I/O runs outside the reducer and
returns completion events tagged with stable IDs and generations.

```ts
while (!stopped) {
  const event = await events.next();
  const transition = reduce(state, event);
  await persist(transition); // Required durability before external effects.
  dispatchEffects(transition.effects); // Completion handlers enqueue events.
  dispatchReadyConversations(); // Launch requests, never await them here.
}
```

Use a deduplicated ready queue with round-robin service, one outstanding entry
per conversation, and a configurable global model-request limit. `continue`
returns the conversation to the tail. Limit tool concurrency separately;
waiting for a job or resource lock consumes no model slot. Make resource locks
worker-owned and stable across model-client/configuration replacement. Preserve
conversation memory isolation and exclusive access to the shared filesystem
and account-wide server mutations.

A dispatched tick captures the inbox watermark, source versions, continuation
generation, observed job results, permissions and cancellation generation.
Normal new messages do not invalidate it: they are for a later tick. An edit or
retraction to its source, permission loss or explicit cancellation increments
the relevant invalidation generation and aborts dependent inference/jobs where
possible. Ignore late results from obsolete model attempts. Independently
record external effects that may already have happened.

Recheck source dependencies and current authority before releasing pending
publication or launching a dependent action. No local check can atomically
prevent a source edit racing a send already in flight; reconcile that race and
retain confirmed delivery. Do not treat cancellation as rollback.

Group context preserves every speaker's stable identity, message IDs, mentions,
reply relationships and chronology. A tick may emit zero, one or several
messages, either plain or targeted replies. The same output structure works in
DMs. Typing/activity is a separate presentation policy; quiet memory maintenance
need not announce itself as typing.

## Durability and recovery

Persist accepted input/watermarks, the tick sequence/latest attempt and decisions,
continuations, tool intents/results, source dependencies and the keyed output
outbox. A self continuation shares the decision's commit. Streaming text and
partial tool arguments remain provisional.

Advance the transport scan cursor once input is durably accepted or intentionally
ignored. Track model consumption separately: commit the tick decision and its
consumed inbox watermark together. An inference error does not consume input.
Delivery acknowledgement is a third independent checkpoint. This removes the
need to hold input scanning behind an unfinished answer while retaining replay
safety. Bound inbox storage; stop advancing beyond unaccepted input when full
and report retention gaps explicitly.

On restart:

1. Reconcile server state and invalidate obsolete sources/permissions.
2. Resume committed decisions, continuations and keyed delivery without rerunning
   the model. Due one-shot timers coalesce into one wake, not missed-interval replay.
3. Mark interrupted model attempts and retry them under the same bounded policy.
   No actions exist for an uncommitted model response.
4. Recover keyed server actions from receipts, retaining the existing receipt
   expiry rules. Never issue fresh keys to resolve ambiguous effects.
5. Mark interrupted local subprocesses/external jobs as interrupted or uncertain.
   Do not automatically rerun arbitrary scripts or writes. Deliver that terminal
   status to an awaiting conversation; the model can inspect evidence next tick.

Jobs survive ticks as durable records; processes are not assumed to survive a
worker restart. Successful results are not rerun. Retain each unobserved result
until a committed tick consumes it, with size limits and explicit truncation.

## Bounds and failure behavior

The model chooses continuation; the host enforces permissions, deadlines and
budgets. Give each externally admitted work episode a configurable tick/token/
tool budget and wall-clock deadline. Self wakes and job completions do not reset
that budget. A new message cannot erase budget already spent by existing work;
admission of additional work is separately rate-limited. These limits also stop
bot-to-bot exchanges from creating unbounded automatic inference.

On budget exhaustion, stop self continuation, retain useful results, and record
a structured stop reason. Apply explicit job deadlines and cancellation policy;
do not leave an unattended job running indefinitely. Retry transient provider
failures with backoff and jitter; permanent errors and exhausted repair/retry
budgets remain visible to operators. Provider retries and repairs consume tick
budget. Do not reset job deadlines on every model tick.

Validate delays against operator limits. Stale timers carry continuation IDs and
are ignored. Duplicate events must not create extra model calls or effects.
Logs expose IDs, wake causes, queue delay, request duration, budget usage and
terminal states without logging credentials, prompts or message bodies.

## Changes from the current worker

- Split `ModelClient.complete()` into a one-request adapter and scheduler-owned
  continuation. Remove its hidden multi-request tool loop and automatic repair/
  retry calls; every inference request becomes an explicit tick attempt.
- Replace global `Agent.drain()` with per-conversation state and a fair scheduler.
- Replace message-owned tasks/group leaders with inbox watermarks and explicit
  tick source dependencies. Keep stable identities and revision reconciliation.
- Promote turn-local `ToolJobs` into worker-owned jobs with durable intents and
  completion events. Replace model-side `wait_jobs` with scheduler suspension.
- Generalize group `finish_turn` into `finish_tick` for rooms and DMs. Separate
  deciding to speak from deciding to continue.
- Preserve the typed Bot API client, native tool implementations, memory and
  filesystem boundaries, receipt recovery and keyed outbox behavior.

Worker KV format v2 includes a tested migration from validated v1 records. Preserve pending keys, uncertain actions
and output payloads. Reject incompatible data rather than silently baselining
over accepted work.

## Acceptance scenarios

1. One DM starts tick A; A starts a slow job and yields. Another conversation
   gets a model slot immediately. The job finishes, tick B answers and waits.
2. A room burst produces one initial tick; the model can stay silent or request
   more ticks without another mention. No input or continuation means no calls.
3. A sleeping three-second room batch does not delay a DM or direct mention.
4. New input during inference remains visible for a later tick. At most one
   model request per conversation is active at any time.
5. A job completes during decision commit or before `on_jobs` registration;
   its result still triggers exactly one eligible wake.
6. `on_jobs(any)` consumes its wake once. A later completion does not wake the
   model unless its latest continuation watches it.
7. A new message supersedes a pending continuation. The old timer/completion
   cannot start a duplicate tick; running jobs remain visible.
8. Silence, duplicate notifications, own output and reconnects do not restart
   completed work. A self-continuation chain terminates at its configured bound.
9. Edit, retraction and cancellation invalidate dependent pending output; late
   inference cannot start tools. Confirmed effects remain recorded.
10. Restart at each commit/dispatch/acknowledgement boundary preserves input,
    decisions and keys, avoids duplicated known effects, and surfaces uncertainty.
11. Malformed responses commit no effects; each bounded repair is a recorded
    tick. Provider failures cannot hold another conversation's model slot.
12. Local control commands remain responsive during inference and tool waits.
