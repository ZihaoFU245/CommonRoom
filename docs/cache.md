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
