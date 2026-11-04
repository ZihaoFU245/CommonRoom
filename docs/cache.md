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

The defaults are 32 MiB of conservative history accounting, 10,000 message
entries, 256 pages and 1,000 entries per conversation. Unread results default to
4,096 entries. Serialized snapshots default to 32 MiB and 128 entries by
account/profile and state, with authorization checks on reuse.

Configure these application caches under `server.cache` in `config.json` on
either database backend; changes require restart. Omitted fields keep defaults:

```json
{
  "server": {
    "cache": {
      "history": {
        "max_bytes": "32m",
        "max_messages": 10000,
        "max_pages": 256,
        "per_conversation": 1000
      },
      "unread_entries": 4096,
      "snapshots": { "max_bytes": "32m", "max_entries": 128 }
    }
  }
}
```

History byte limits accept integer bytes or binary `k`/`m`/`g` units, from
4 KiB to 1 GiB; messages accept 4–100,000, pages 4–4,096, and per-conversation
entries 1–100,000. Unread entries accept 4–100,000. Snapshot byte limits accept
4 KiB–1 GiB and entries 1–4,096. The four history/unread shards each receive
one quarter of their respective global budgets, rounded down; shards cannot
borrow unused capacity. Recovery preserves configured limits.

Startup migrates the previous `database.history_cache` object to
`server.cache.history` and saves the normalized configuration. Specifying both
locations is rejected before configuration is rewritten. Other cache fields
retain defaults during migration. Duplicate JSON keys (including escaped spellings
of the same key) are rejected at every nesting level before migration or rewriting.

These are independent cache budgets, not total RSS caps. Response-held pages,
metadata templates, database caches and temporary allocations use additional
memory. Metadata authority templates retain their own internal bounds.

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
metrics use headings, overall outcome/cache percentages, capacity utilization,
cache behavior, ordering waits and per-route latency tables. Both formats include
the complete raw counters and a Unix collection timestamp after the summaries,
including shard occupancy and per-route histogram buckets. The default report
formats these details as Markdown tables; `--txt` places the same information in
one plain-text code block for sharing or interval comparisons.
In the browser, metrics sections are collapsed by default and expand independently
through native disclosure controls. The title and hit-rate summary stay visible;
console and bot responses retain the complete Markdown report.

Bot command receipts retain the system authority used to issue a report. Both
replay and receipt lookup recheck that authority, so revoked command/sudo access cannot
retrieve an earlier report. The authority marker is stored in the receipt JSON.
SQLite HEAD is v21 (with the v19
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
  microseconds and nine non-cumulative latency buckets (<=0.5 ms, <=1 ms, <=5 ms,
  <=10 ms, <=50 ms, <=250 ms, <=1 s, <=5 s, >5 s). Labels use registered route patterns; unknown paths share
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
is counted; there is no sampling. Finer buckets add no extra atomic updates per
request: completion still increments exactly one bucket. Percentages, averages,
aggregate route statistics and p95 bounds are calculated only on report demand.
The displayed p95 is the upper bound of the bucket containing the 95th percentile
(or `>5 s`), not an exact percentile. Empty denominators and maximum timings
without completed samples display `n/a`. A measured zero-duration sample still
displays `0.000 ms`. HTTP success means a completed non-4xx/5xx response; error percentages use completed responses,
while cancellation percentages use completed plus cancelled attempts. The overall
latency percentages use the histogram sample count. No interval timer, automatic
metrics log, unbounded sample list or per-user label is introduced. Concurrent
reports can observe different instants across counters, so fields such as completed counts and bucket sums can briefly
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

The capacity summary includes current browser/bot socket utilization, tracked
HTTP/I/O peaks, and the highest history shard utilization for bytes, messages and
pages. Aggregate spare capacity can coexist with a full shard because shards do
not borrow budgets. Cache behavior reports eviction counts and miss-reason shares;
ordering summaries show average/maximum wait and timeout percentages. These values
are derived from existing counters only when the command runs. The report itself
occupies request/job capacity. Host CPU, process RSS, detailed S3 phase timings and
end-to-end browser latency are outside this report; backend route timing includes
awaited S3 work but excludes subsequent browser downloads.
