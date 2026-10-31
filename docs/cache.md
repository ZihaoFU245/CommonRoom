# Server caches and system metrics

The server shares history storage across viewers and database connections. Cache
capacity does not multiply with reader-pool size. SQLite and PostgreSQL use the
same application caches; their database page/buffer caches are separate.

## Read reuse and committed writes

History pages are immutable shared buffers, keyed by conversation identity,
revision and range. Matching requests reuse bodies and at most two encoded JSON
ranges per page. Concurrent cold requests share a fill. A covered bounded
sequence interval can reuse a slice of a larger page without another query,
allocation of message bodies, or duplicate cache entry.

After a known committed native send is published, existing latest-page entries
are rebuilt from their cached messages plus the committed message. They keep
their requested count, byte budget and retention bounds. Historical ranges are
invalidated on appends because retention can remove rows outside the cached tail.
Edits/reactions replace affected immutable pages and carry unaffected pages to
the new revision. Previously returned responses keep their original immutable
content. Serialization sizing runs once for the changed message only when an
existing page needs it; cached sizes are reused for unchanged messages.

Edits/reactions preserve each cached user's unread count/first/retention
coordinates while advancing the revision. Read-cursor mismatches still miss;
appends use the existing SQL incremental-delta path. Metadata changes and
deletions retain conservative revision invalidation. Failed or unknown commits
never refresh caches. Identity/permission and database health are rechecked
before cached content is delivered.

The default history budgets are 32 MiB of conservative accounting, 10,000 message
entries, 256 pages and 1,000 entries per conversation. Four shards each receive a
quarter of global budgets and cannot borrow unused capacity from another shard.
`database.history_cache` tunes the global byte/message budgets. Unread entries
have a separate 4,096-entry limit. Serialized snapshots have a separate fixed
32 MiB/128-entry cache by account/profile and state, with authorization checks on
reuse. Response-held pages, metadata and database allocations are additional
memory. Neither configured budget is a total RSS cap.

## Commands

```text
/system
/system configs
/system metrics
/system metrics --txt
```

All subcommands require a global `/system` command grant (included for su).
Use `/grant alice @global /system` to delegate read access. Stdin and authorized
`/sudo /system ...` are supported. Action grants alone do not enable commands. `/configs` is not executable. Its persisted
permission name remains readable for existing HEAD data, without authorizing any
new system command. Outputs are Markdown; configuration is fenced JSON and
metrics use headings, summary rates and tables. Add `--txt` to return the complete
metrics report as plain text in one Markdown code block for sharing.
In the browser, metrics sections are collapsed by default and expand independently
through native disclosure controls. The title and hit-rate summary stay visible;
console and bot responses retain the complete Markdown report.

Bot command receipts retain the system authority used to issue a report. Both
replay and receipt lookup recheck that authority, so revoked command/sudo access cannot
retrieve an earlier report. The authority marker is stored in the receipt JSON.
SQLite HEAD is v20 (with the v19
credential upgrade); PostgreSQL remains at its independent v1 schema.

`/system configs` shows configuration only. `/system metrics` reports cumulative
operational counters since startup plus current occupancy and worker capacity.
Eviction and recovery do not reset counters. Simultaneous observations can differ
slightly because reporting does not stop traffic or acquire a global lock.

## Metrics

History page hit rate is `hits / (hits + misses)`. Requests that reuse a concurrent
fill count as hits. Counters describe cache/database-load attempts, not HTTP
success rates. A large around-message lookup may require more than one load.
For a current workload, compare changes in counters between two reports rather
than relying only on lifetime averages. A zero denominator displays `n/a`.

| Counter | Meaning |
| --- | --- |
| `events.cold_misses` | No retained page for this conversation at the final lookup. Includes first reads and fully evicted caches. |
| `events.revision_misses` | Pages exist for the conversation, but none at the requested revision. |
| `events.range_misses` | The requested revision exists, but the requested range is not covered. |
| `events.coalesced_hits` | A hit after the fill coordinator rechecks the cache. |
| `events.subset_hits` | A bounded interval served from a larger cached page. |
| `events.tail_updates` | Existing tail pages refreshed after committed appends. |
| `events.updated_pages` | Cached pages rebuilt after committed appends/edits/reactions. |
| `events.carried_pages` | Unaffected page contents carried to a new edit/reaction revision. |
| `events.invalidated_pages` | Entries removed after a revision change or recovery. |
| `events.oversized_skips` | Entries too large for their shard's byte/message budget. |
| `events.stale_fills` | Late fills rejected because a newer conversation revision is cached. |
| `events.*_evictions` | Entries evicted for byte, message, page or per-conversation limits. When multiple limits apply, one reason is counted per removed entry. |
| `shard_stats` | Current pages/messages/bytes and effective limits for each shard. |
| `unread_hits`, `unread_misses` | Exact revision/read-cursor lookup results; these define unread hit rate. |
| `unread_full`, `unread_incremental` | Full versus incremental SQL calculations after misses; neither is an unread cache hit. |
| `unread_carried` | User entries whose valid unread result survived an edit/reaction. |
| Snapshot `hits`, `misses`, `coalesced` | Baseline reuse, new entries and reuse of an in-progress fill. |
| Snapshot `builds`, `failures`, `incomplete` | Completed builds, failed builds and partial baselines that were not retained. Cancelled builds are excluded. |
| Database pools | Open/idle/minimum/maximum sessions and completed checkout attempts, errors/timeouts, total/max acquisition microseconds. |
| Database `operations` | Successful/failed completed native history reads, write transactions and cursor advances. These are operation results, not per-statement profiling; acquisition failures and cancelled operations are excluded. |

Pool acquisition time includes checkout health checks and connection creation,
not just queue wait. Existing duration counters are reused; no extra statement
timing, profiling, tracing of message bodies, or per-user metric maps are added.
Collection uses integer counters while cache locks are already held and the
existing relaxed pool counters. Reports allocate/format only on command demand.
There is no periodic metrics task or automatic metrics logging.

Profile-only changes preserve authority templates and patch their cached avatar
values within the existing profile/byte budget. Serialized baselines still receive
a new delivery epoch, and their final authorization checks remain mandatory.
Policy, identity and membership changes invalidate authority templates normally.
See [Storage benchmarks](storage-benchmarks.md) for cache and engine-inspection
measurement procedures and work-count assertions.

## Admission and request metrics

The Server admission section reports configured job/HTTP limits, available job and
socket permits, active authenticated API reservations, and effective account/socket
limits. `admission.http` contains bounded lifetime counters:

- `connections`: accepted HTTP connections, current/peak active connections,
  connection errors and header/write timeouts. Upgraded WebSockets leave HTTP
  occupancy and are tracked by their separate socket permits. Connection errors
  include transport/protocol failures and timeout failures; timeout counts can overlap.
- `io_jobs`: accepted general jobs and current/peak retained job permits. A job
  stays active after HTTP cancellation until its accepted work completes. Console
  jobs also use this budget; internal recovery's direct reservation is reflected
  in available permits rather than these job counters.
- `rejections`: account HTTP, general I/O, password and socket capacity rejections.
  `socket_replacements` counts account/profile socket replacement. Password
  rejections share one counter across login, credential commands and stdin provisioning.
- `body_timeouts`, `body_rejections` and `request_timeouts`: absolute body deadlines,
  oversized/invalid body reads, and whole-request response-creation deadlines.
- `routes`: active/completed/cancelled requests, 4xx/5xx counts, completed total/max
  microseconds and six non-cumulative latency buckets (<=10 ms, <=50 ms, <=250 ms,
  <=1 s, <=5 s, >5 s). Labels use registered route patterns; unknown paths share
  `unmatched`. No user IDs, query strings, credentials or message bodies are stored.
  Time includes body receipt, admission, handlers and security checks, but excludes
  header receipt, response transfer and the bot error-envelope wrapper. WebSocket
  request timing ends at upgrade, not disconnection. HTTP status errors overlap
  timeout/rejection counters. Cancelled requests do not contribute completed timing.

Route counters and latency buckets use relaxed atomics. The route table is built
alongside browser/bot route registration and frozen before serving requests. Request
start performs an immutable lookup; completion/cancellation updates the retained
route counters directly. Neither path takes a metrics mutex or allocates a route
label. Report collection reads atomics without blocking these updates. Every request
is counted; there is no sampling. Concurrent reports can observe different instants
across counters, so fields such as completed counts and bucket sums can briefly
differ while traffic continues; they agree after the updates finish. Untouched
routes are omitted. Existing admission/cache locks retain their own responsibilities.

`ordering` reports completed permit-acquisition attempts, timeout count, and total/max
microseconds across conversation sets, metadata fences and audit permits, with
separate `conversations`, `metadata` and `audit` breakdowns. Total time
includes timed-out attempts. Cancelled acquisitions and authoritative recovery's
unbounded fence acquisition are excluded. These metrics measure pre-execution wait,
not transaction duration. Reports themselves occupy request/job capacity; current
occupancy is not expected to be zero while a report is collected.

Use deltas over a representative traffic interval. Low cache reuse alone does not
prove wasted work: writes, permission changes and cursor changes can require a fresh
snapshot even when its history pages are reused. No cache/pool increase is warranted
by a low reuse percentage without evictions, slow acquisition or observed latency.
