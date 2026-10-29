# PostgreSQL review verification and fixes

## Current memory and retention follow-up

The later pool-memory review found two reproducible issues beyond the historical
ledger below. PostgreSQL now bounds candidate page payloads before assembly and
decoding, transmits each reaction label once per message, and updates deletion
retention once per statement/conversation. [Storage benchmarks](storage-benchmarks.md)
records the before/after SQL measurements and regression commands.

PostgreSQL stays at schema v1 because it is unused and unpublished. Fixes modify
v1 directly; there is no PostgreSQL schema-upgrade path. Unsupported schema
markers remain rejected unchanged. SQLite stays at v19 and read-only HEAD SQLite
transfer targets current PostgreSQL v1.

Optional history-cache byte/message budgets and PostgreSQL idle timeout settings
are validated before bootstrap/config rewriting. `/configs` exposes effective
cache budgets and pool sizes/acquisition timing/errors. Debug storage traces add
read/write-operation durations without query parameters or message bodies.

Validation: `auto/check.sh` passed (413 ordinary Rust tests and 309 web tests,
plus formatting, Clippy, web type/lint/format and harness syntax checks). All
59 PostgreSQL contracts passed in disposable schemas, including new raw-payload,
bulk-trigger and idle-reclamation contracts. The real-binary
SQLite-to-PostgreSQL transfer, sessions/passwords, HTTP restart/shutdown and
second-owner configuration checks passed with unchanged SQLite source bytes.
Working `data/` and `pgdata/` were not used.

## Historical review

This records verification of `analysis.md` against the current implementation.
The review mixes reproducible defects, intentional contracts, latent hardening,
and performance estimates. Findings were checked against call sites, existing
regressions, and the tests described below. The original review is preserved.

## Applied fixes

| Findings | Resolution and evidence |
| --- | --- |
| S-1 / B-1 | Owner probe deadlines return a retryable failure without permanently disabling the dispatcher. Known rejected commits (constraint, transaction abort, lock timeout/cancellation, SQLite BUSY/LOCKED) clear the ambiguity latch. Transport errors, SQLSTATE 40003, disk I/O errors, and cancellation during an unknown commit still fail closed. A deferred SQLite foreign-key rejection proves rollback and subsequent writes work. |
| S-2 | Ownership validation checks the actual session advisory lock through `pg_locks`. A healthy connection whose advisory lock was explicitly released is rejected. The owner never reconnects into potentially stale in-memory state. |
| S-3 | Bot authorization now requires an explicit `Scope::Global` or `Scope::Conversation(id)` at every call site. Operation restrictions continue to apply to both. Existing scoped HTTP/socket/command regressions pass. |
| B-8 | Startup cleanup applies the pending grace period to unreferenced uploads. Fresh pending and completed uploads survive restart; stale or previously claimed objects remain eligible. Restart/quota regression also completes an upload after restart before aging it for cleanup. |
| B-10 | Metadata revision updates include the expected revision and reject stale/missing rows. Message revision updates also use CAS and require exactly one affected row. Metadata and message stale-write tests prove rejection without overwriting newer state. |
| B-11 | Ready and abandon SQL binds the authorized uploader. A wrong-uploader request cannot complete or abandon another record. Live metadata is already stable through the shared mutation fence; reduced quota cannot race completion through the administrative path. |
| B-12 | Command receipt insertion must affect one row, preventing an effect from committing without its receipt if the account precondition is violated. |
| B-14 | Synchronous sudo phases catch unwinding, restore the read model and persisted authorization, then resume unwinding. A panic-injection regression proves temporary superuser authority is removed. |
| B-17 | Avatar completion authenticates before taking the object lock, then reloads under that lock. Completion of an already-ready upload is terminal and cannot restore a subsequently replaced/removed avatar. |
| B-19 | Login accounting distinguishes success, credential failure, and infrastructure/conflict failure. Infrastructure errors refund only their pending reservation and preserve existing failure/backoff state. A concurrent-failure regression verifies the next delay still grows. |
| B-20 / R-8 | Expired but recognized stored sessions generate a logout audit event and account change when removed. Unknown cookies still produce no event. A repeated logout regression verifies exactly one event. |
| B-20 admission | Command workers inherit the HTTP account lease explicitly; nested command admission reuses it. WebSocket commands still acquire their own lease. A full-capacity regression proves the advertised eight slots remain available and release correctly. |
| E-1 | Successful read/cache health checks share one result for at most 100 ms, with single-flight rechecking after the owner mutex. A warm read does not wait on that mutex. Transaction writes retain immediate precommit ownership validation; acquisition now shares cached health. Autocommit cursor writes force validation immediately before their statement. |
| E-2 | Snapshot tails load with a bounded ordered stream of four queries. Existing per-viewer authorization, shared immutable history pages, output budgets, partial results, and final validation remain intact. |
| E-3 | PostgreSQL readers default to 2 through 8 with configurable limits; SQLite uses 2 to 4. PostgreSQL page revision/anchor/messages/reactions/bounds use one statement snapshot without BEGIN/ROLLBACK round trips. Pagination, large-message budgets, NUL/Unicode preservation, and native transport contracts pass. |
| E-4 | PostgreSQL unread requests use one set-based statement for the entire batch, retaining incremental append/read-cursor deltas and full fallback on edits/deletions. The SQL oracle and full PostgreSQL suite pass. Full fallback still scans the unread range; no unbounded per-user counter table was introduced. |
| E-8 | PostgreSQL includes the partial account-ID index in its fresh current schema. The earlier SQLite index and v19-to-v20 upgrade were unnecessary and have been removed. SQLite stays at published HEAD v19; PostgreSQL uses its independent current v1 format. Query-plan tests verify the PostgreSQL index. |
| R-6 | Startup reconstructs retention when the entire derived table is empty, on both backends. Normal startup checks table existence of rows without scanning history. Tests simulate a data-only restore and verify reconstructed counts. Partial corruption still requires operator repair. |

## Corrections and retained contracts

| Findings | Verification |
| --- | --- |
| S-1 / B-1 cancellation | `web/commands.rs::execute_input` already spawns accepted writes. Bot, media, cursor, session, metadata, and console work similarly retains accepted work through publication. An ordinary client disconnect does not cancel their COMMIT. Existing PostgreSQL cancellation contracts demonstrate this. Unconditional recovery after an actually unknown outcome would be unsafe. |
| B-2 | Browser first-DM preparation occurs after acquiring its deterministic conversation permit and is repeated against live metadata. The second caller sees the newly published directory. Opposite-direction first-DM tests already verify one directory and both messages. |
| B-9 | Native bot message replay lookup runs after acquiring the conversation permit. Identical requests include the same conversation in their digest, so a concurrent retry waits and then replays. Requests in different conversations sharing a key have different digests and correctly conflict. Existing same-key/changed-input and cancelled-send transport contracts pass. |
| B-3 | Publication mismatches are invariant violations under the metadata/conversation permits, not demonstrated ordinary races. Discarding the fail-closed latch or applying an arbitrary stale delta could corrupt authority or memory. Existing cancellation/revocation tests prove accepted publication finishes before queued metadata writes. Known durable mismatches now trigger the authoritative recovery described below; unknown outcomes remain fail-closed. |
| B-4 | Unavailable errors persist neither effects nor terminal receipts. Domain rejection commits `PendingCommand::None` and its rejection together. It cannot commit a prepared effect and record an unrelated rejection through this branch. Persisted domain failures intentionally replay; changing input requires a new key. |
| B-5 | Browser no-op edits prepare no write; bot reaction no-ops likewise skip storage. Native rows count physical changes, while a directly submitted checked operation may still advance a revision. No externally observable lost mutation was demonstrated. |
| B-6 | Reconciliation iterates `ids.difference(previous)`. Retained attachments deliberately skip revalidation so edits/reactions can preserve expired placeholders. Newly added references undergo live SQL validation. Existing editor-upload and expired-retained-reference contracts cover both cases. |
| B-7 | Concealing absent/foreign/pruned message identity is intentional. Message-gone and history-revision failures already have different domain mappings. |
| S-4 / B-12 locking | PostgreSQL SELECT EXISTS with FOR UPDATE does lock matching rows. Keeping that lock through reference insertion is material protection against cleanup/abandon, including SQL callers. It is retained. |
| B-13 | No reachable reverse row-lock ordering was demonstrated. Metadata administration excludes conversation mutations through the global fence. Native driver errors map to retryable unavailable (503), not the claimed generic 500. |
| B-15 | Profile avatars have an independent 64 KiB cap, quota, pending limit, rate limit, and completion validation. Ordinary users do not have a server-wide attachment upload grant. Requiring that grant would disable avatars for them and conflate profiles with conversation upload policy. This contract is now explicit in Security. |
| B-16 | `include_expired=true` deliberately exposes retained expired attachment placeholder metadata after object deletion. It never grants downloads, still requires current conversation read access and a retained reference, and returns not found after retraction. Existing engine/HTTP/browser regressions and Testing specify this behavior. |
| B-18 | All-or-nothing multi-conversation cleanup retains matching target permits through commit and publication; counting now runs outside permits with full revision revalidation. Committing per conversation would permit partial deletion/audit on failure. Acquiring the metadata fence before conversation locks can deadlock against existing writers. Atomicity and queued-metadata regressions retain that lock order. The revised count/revalidation regression also covers a new message in a previously empty target. |
| B-20 publication | Publishing committed state before the final credential response check is required when a credential expires during I/O. Metadata revocation cannot interleave through the held fence. Cursor work is already detached through publication. Profile attempt budgets intentionally bound attempts, including failed persistence. |
| E-5 | `projection.rs` stores only indexed routing/reference fields, not a second message body. The original JSON text preserves embedded NULs that JSONB cannot represent. A generated `json::jsonb` would reject supported content. Transaction counts remain an optimization opportunity; the claimed two copies of each body are absent. |
| E-6 | A PostgreSQL sequence could expose a later committed cursor while an earlier append is uncommitted, causing incremental readers to skip that later-arriving message permanently. Transactional counter ordering is required and explicitly tested; it is retained. |
| E-7 / E-9 | Uncontended memory operations now use short async-side try-lock guards, with blocking fallback on contention. Media cleanup acknowledgements are batched while retaining object locks and cancellation-safe completion; see I-6/I-8 below. |
| E-10 | `MessagePage::clone` already clones an Arc and range, not message bodies. SQLx's PostgreSQL prepare path uses its statement cache. SQLite reader cache sizing remains a tuning choice, not a demonstrated correctness issue. |
| R-1 / R-2 | Backend selection and empty-target checks are intentional fail-closed operational requirements. Typed status/code/retryable fields are the client contract; consumers should not parse diagnostic text. Configuration/transfer failure tests remain. |
| R-3 / R-4 | Master already used global `data.next_sequence` and S3-backed media/avatar objects. These are not regressions introduced by this branch. |
| R-5 | Accepted work is detached deliberately and retains bounded I/O/account admission. The duplicate-account-reservation issue is fixed above. |
| R-7 | `authenticated_identity` already propagates `store.health().await?`; database failures map to 503 through `EngineError`, not 401. Credential verification failures remain 401. |
| S-5 / R-0 | Bounded audit/receipt retention and relocated validation checks remain in place. No actionable authorization regression was demonstrated. |

## Compatibility and validation

SQLite stays at HEAD v19 with no schema upgrade. Unpublished PostgreSQL uses an
independent v1 marker and includes current indexes at creation; old WIP formats
are rejected. There are no PostgreSQL WIP upgrade paths. Transfer always requires
HEAD SQLite and creates current PostgreSQL without rewriting source bytes. See
[Data](data.md) and [PostgreSQL](postgresql.md).

Validation of the initial `7e043d7` fixes (before the follow-up below):

- `auto/check.sh` passed: formatting, strict Clippy, 403 ordinary Rust tests
  (53 opt-in tests ignored), worker checks/tests, web type/lint/format checks,
  and all 300 web tests.
- All 50 PostgreSQL contracts passed serially in unique disposable schemas.
- The explicit v19 offline-transfer regression passed with unchanged source
  bytes and preserved account/session/history/cursor/receipt data.
- Debug and release builds passed the complete HTTP/WebSocket/bot smoke harness.
- Isolated real RustFS media smoke passed, including pending-upload preservation
  after restart, avatars, private reads, cleanup, outages, and recovery.
- Real-binary SQLite-to-PostgreSQL transfer, HTTP sessions/passwords, restart,
  graceful shutdown, ownership rejection, and configuration preservation passed.
- `git diff --check` passed. Working `data/` and `pgdata/` were not used by tests.

The final follow-up below addresses E-7/E-9 and shares heavy snapshot content.
Personalized authorization/unread projection remains per viewer; persisted
per-user unread counters were not introduced.


## Follow-up on the revised action list

The revised `analysis.md` was rechecked after `7e043d7`. The publication status
clarification supersedes the previous v20 migration decision: published SQLite
remains v19; unpublished PostgreSQL has its independent current v1 format.

| Item | Result |
| --- | --- |
| I-1 | The cited `save()` calls already execute inside `tokio::spawn` workers, not directly in the cancellable handler. A new HTTP regression uses a deferred trigger to block inside COMMIT, aborts the HTTP future, then verifies profile publication, authentication and a subsequent write succeed. Existing media/profile/credential/conversation cancellation contracts remain. |
| I-2 | Superseded: both historical schema upgrade paths were removed. SQLite stays v19 without the PostgreSQL-only account index; PostgreSQL creates its full current v1 schema in an empty target. No `IF NOT EXISTS` migration workaround is needed. |
| I-3 | PostgreSQL retention rebuild uses `SET LOCAL statement_timeout=0` inside its startup transaction. A slow-trigger regression exceeds a 20 ms runtime deadline during reconstruction and verifies that deadline is restored afterward. Offline HEAD transfer already disables request deadlines on its dedicated import connection. |
| I-4 | Write acquisition now uses cached health. Root transactions keep the forced precommit check; cursor autocommit writes force a check immediately before execution. The ownership test holds the owner mutex while both cached reads and writer acquisition progress, then verifies explicit ownership loss still fails closed. No unmeasured throughput improvement is claimed. |
| I-5 | Concurrent cache misses share one page load per conversation/revision/range. Immutable pages cache at most two encoded ranges, reused across viewers. A 100-viewer/30-room fixture asserts 30 cold page reads and one read after changing one room, shared page storage and one encoding per page. Personalized authority, cursors and unread projections remain distinct. |
| I-6 | Uncontended memory capture/publication executes with a short async-side try-lock guard; only contended access dispatches to the blocking pool. Thread-identity tests cover the uncontended path, and the real panic regression retains fail-closed poisoning. No engine guard spans an await. |
| I-7 | Count runs outside conversation permits. Only matching conversations are locked, followed by complete target/revision revalidation including empty targets. Changes retry at most three times. Delete/audit commit atomically. A PostgreSQL test blocks counting, appends to an empty target, then verifies recount and one complete deletion/audit. |
| I-8 | Successful object deletions and staging deletions are acknowledged with one batched transaction. Object locks remain held through detached acknowledgement; failed objects stay claimed and quota-reserved. SQLite and PostgreSQL rejection tests prove batch rollback and selective successful acknowledgement on retry. |
| I-9 | A known durable publication mismatch queues one authoritative metadata/cursor reload under the exclusive metadata fence. Validation/compilation precede installation, caches/indexes/clocks rebuild, and clients receive resynchronization. SQLite/PostgreSQL regressions verify a subsequent write; invalid reload remains unavailable. Shutdown drains queued recovery. Unknown commit outcomes, ownership loss and poisoned locks still require restart. |
| I-10 | Documented the deliberate fail-closed poisoning policy in Security and added a panic through the real ordered worker. Subsequent engine reads and mutations must return unavailable. Recovering with `into_inner` is prohibited because arbitrary command state may have been partially mutated. |
| I-11 | Defaults are 2–8 PostgreSQL readers and up to 8 writers, plus ownership. Optional database.postgres_pool validates min_readers/max_readers/max_writers before bootstrap or config rewriting. Tests verify invalid settings preserve config and actual pool options honor selected limits. |

The persistent transfer contract now asserts the SQLite HEAD source marker and
the independent PostgreSQL target marker, while retaining byte-for-byte source
preservation, verified copy, rollback, session/password/receipt/cursor retention,
restart and second-owner protections. Non-HEAD SQLite (including WIP v20) is
rejected before connecting to the destination or rewriting source bytes.

Follow-up validation:

- `auto/check.sh` passed: strict formatting/Clippy, 404 ordinary Rust tests
  (54 opt-in ignored), worker checks/tests, web types/lint/format and 300 web tests.
- All 51 PostgreSQL contracts passed in isolated schemas, including actual
  mid-COMMIT HTTP cancellation and the slow retention-rebuild regression.
- Rebuilt debug/release binaries passed the HTTP/WebSocket/bot harness.
- The real-binary transfer harness passed HEAD SQLite v19 → PostgreSQL v1,
  unchanged source bytes, sessions/passwords, restart and ownership checks.
- The isolated RustFS media/avatars/restart/outage smoke passed.
- `git diff --check` passed. Working `data/` and `pgdata/` were not modified.


## Final remaining-issue verification

All eleven revised items are addressed by implementation, supersession, or the
explicit tested fail-closed contract above. One forced ownership probe per append
is asserted across 50 PostgreSQL appends; no unmeasured 2× throughput claim is made.
The local SQLite fan-out fixture measured 563 ms cold and 487 ms after one room
changed, including assembly/serialization; read-count and sharing assertions are
the stable contract, not these machine-dependent timings.

Dependency advisory CI now audits the resolved locked workspace graph with all
workspace features and target platforms. The local fresh RustSec run passed for
284 packages against 1,380 advisories. Thirty inactive lockfile entries were
excluded, including the unused SQLx MySQL/RSA dependency affected by
RUSTSEC-2023-0071. No advisory is suppressed; enabling that dependency includes it
automatically. See [Testing](testing.md) and `auto/audit.py` for scope and commands.

Final validation also includes 56 PostgreSQL contracts in disposable schemas,
real-binary HEAD SQLite v19 → PostgreSQL v1 transfer with unchanged source bytes,
debug/release HTTP/WebSocket/bot smoke, and isolated RustFS media smoke. The
working `data/` and `pgdata/` directories were not used.

Final `auto/check.sh` passed: formatting, strict Clippy, 410 ordinary Rust tests
(60 opt-in ignored), optional worker checks/tests, web types/lint/formatting and
all 300 web tests. The final fan-out assertions passed again after the cache-key
refinement. `git diff --check` passed.
