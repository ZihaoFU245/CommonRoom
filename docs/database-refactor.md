# Database refactor

The target is one asynchronous, typed storage interface with two independent
implementations. SQLite and PostgreSQL each own their SQL, connection management,
transactions, locking and schema mechanics. SQLx is the driver for both; there is
no SQL translation layer and no requirement for PostgreSQL queries to fit SQLite.

SQLite serializes writes through one writer and uses separate readers. PostgreSQL
uses a bounded pool of concurrent write transactions and independent readers.
The database ownership advisory lock requires a dedicated PostgreSQL session;
it must not be returned to a transaction pool. Multiple chat processes remain
unsupported until shared application-state coordination is designed separately.

Operations, rather than arbitrary SQL strings, form the public storage contract.
A mutation owns its complete transaction: content, revisions, retention, media
references and applicable retry receipts commit together. Results carry committed
revisions/effects. Shared page and unread caches sit above backend dispatch;
prepared-statement caches belong to SQLx. Data and revision must be read from the
same snapshot. Completion of an older operation must not regress published state.
No global application lock may serialize all PostgreSQL I/O across an await.

Message sequence allocation is transactional on both backends. The counter
preserves current global ordering/cursor semantics and creates a serialization
point for message appends.
Using a PostgreSQL sequence alone is insufficient: allocation order does not imply
commit order, and advancing a cursor must not skip a later-committing message.

## Current implementation state

This refactor is not complete. The latest WIP checkpoint and resume notes are at
the end of this document; earlier verification sections describe historical
checkpoints. The driver conversion is implemented:

- SQLx 0.8.6 replaces direct rusqlite, postgres and native-TLS dependencies.
- Separate `engine/database/sqlite.rs` and `postgres.rs` implement driver execution.
- SQLx owns prepared-statement caching; the custom PostgreSQL statement map is gone.
- Streaming binary COPY, lossless JSON, HEAD v19 representation, ownership locks,
  bounded reader snapshots and transaction/receipt recovery remain operational.
- Existing synchronous engine callers use the temporary `database/runtime.rs`
  bridge. This bridge is transitional and is not the target async storage API.

A native dispatcher now owns a one-connection SQLite writer pool or an
up-to-eight-connection PostgreSQL write pool with a dedicated ownership session.
Message writes and cursor advances enter typed async operations. Their SQL/binding
lives in separate backend modules. Message payload/reactions, media references,
retention, revisions and receipts remain in the operation's transaction, including
nested bot receipt transactions on explicit command-scoped handles. Message
identity/sequence/revision is returned for publication after commit; outer bot
commands finish their receipt before external publication. Conversation
identity/revision checks prevent stale writes. A transactional global counter
allocates append sequences and remains locked through commit. Pooled sessions
contribute shared mutation diagnostics; cache checks do not acquire a writer lease.

Native pooled readers now provide pages, unread batches, message lookup and cleanup
counts. The shared history layer contains bounded revision-keyed caches, with no
SQL. Metadata/account changes, authentication events, retention, media lifecycle,
and retry receipt reads/writes also enter typed native operations. PostgreSQL
quota checks lock the uploader account; cleanup claims use `SKIP LOCKED`.

Each backend owns its v19 schema and native SQLx startup validation/initialization.
Shared storage contains no production SQL or dialect dispatch. Offline streaming
transfer and fingerprint verification live under `engine/database/transfer.rs`;
the engine supplies only its domain metadata validator. The paired-SQL wrapper
has been removed. The remaining synchronous query adapter serves offline transfer,
legacy transaction control and test injection.

Standalone native writes now use SQLx transactions, including automatic rollback
on cancellation. Pool lease returns run asynchronously on the persistent storage
runtime; shutdown drains all connections before releasing ownership locks. A
commit error or cancellation with an unknown acknowledgement invalidates the
dispatcher on either backend. Bot command transactions have explicit scoped
handles; independent handles never join another command's transaction. The old
dispatcher-global pinned session has been removed.

Browser/bot HTTP snapshots, paginated history and socket notification plans now
await native database operations. Their metadata capture/revalidation uses short
worker guards with no database ping or bridge under the engine read lock.
An async cell shares baseline builds and permits refill after builder cancellation.
Cache misses validate ownership through their reader acquisition; warm hits still
validate ownership without acquiring a reader. Notification revision checks cover
the conversations represented in the batch. Legacy synchronous callers on a
multithreaded runtime no longer start another OS thread for each bridge call.

Browser plain-text/`/tell` sends and `/reply`, `/edit`, `/react`, `/retract`,
`/clean` actions, plus bot HTTP sends/reactions, now prepare owned state and await native writes
and publish committed revisions under short engine guards. Browser/bot read
acknowledgements use the same approach for cursors. A per-conversation permit
orders all three phases and holds a shared metadata fence. Legacy commands take
that fence exclusively before the engine lock, so they cannot revoke authority,
rename resources or alter retention between preparation and committed publication.
Waiting metadata writers do not occupy the engine lock. Cancellation retains
accepted work, admission and the conversation permit through publication.

Receipt capacity and duplicate-key checks are inside native message transactions.
PostgreSQL serializes them on the receipt account row before allocating the
append counter. This keeps capacity atomic across independent conversations.

The remaining required work is removal of synchronous runtime/command control
bridges from the remaining browser slash commands, administrative/account
mutations, outer bot command transaction control and console entry points. Those
operations still hold engine guards over blocking storage calls. Attachment media
reservation, read, completion, abandonment and background cleanup now await native
operations outside engine guards.
Migrated browser text/direct-message, bot message/reaction and cursor operations
do not. Engine memory publishes
the sequence assigned by the database; metadata operations cannot overwrite the
counter. The full application refactor is not complete in this state.

## Verification at the driver-conversion checkpoint

On 2026-10-26, Rust formatting, all-target Clippy with warnings denied, and all
366 ordinary Rust tests passed. All six opt-in PostgreSQL integration contracts
passed on a disposable PostgreSQL 18.6 cluster. The real-binary
`auto/test-postgres.py` smoke passed against the same isolated cluster, including
SQLite provisioning, verified offline transfer, preserved session/password,
PostgreSQL startup/restart/shutdown, second-owner configuration protection and
unchanged SQLite source bytes. No working `data/` database was used.

Completion additionally requires concurrency tests proving independent PostgreSQL
writes can progress concurrently, same-conversation ordering, duplicate receipt
atomicity, monotonic cursors, quota/retention correctness, authorization rechecks,
commit uncertainty/cancellation behavior and cache revision coherence. Existing
SQLite/PostgreSQL contracts and the real-binary migration smoke must still pass
through the final operation interface.

## Verification at the pooled-dispatch checkpoint

On 2026-10-26, two new ordinary tests verify single-writer SQLite capacity,
cancelled acquisition recovery and stale-message rejection. Two PostgreSQL tests
verify a locked conversation does not block another conversation's typed message
write, concurrent cursors remain monotonic, and an idle pool session can be
replaced independently of the ownership session. The existing ownership-loss
regression now terminates the dedicated ownership session rather than an idle
write lease. All eight PostgreSQL integration tests passed on isolated PostgreSQL
18.6. All 368 ordinary Rust tests, all-target Clippy with warnings denied, formatting
and diff checks passed after the publication/health-check adjustments. The
real-binary migration/HTTP smoke also passed again with the native pools.

## Verification at the native-operation checkpoint

On 2026-10-26, 371 ordinary Rust tests and 13 opt-in PostgreSQL contracts passed,
with formatting, all-target Clippy with warnings denied, and diff checks clean.
This includes native metadata/media/receipt operations, schema startup and offline
transfer extraction, database-assigned append counters, receipt cursor agreement,
counter rollback/exhaustion, cancellation with nonblocking pool return and
explicit command-context isolation. Startup continues to accept fresh/v19 data
and reject unsupported data unchanged. SQLx pool shutdown drains connections
before releasing ownership, preserving source bytes during immediate transfer.

The rebuilt real binary also passed `auto/test-postgres.py` against a fresh
disposable PostgreSQL database: provisioning, verified transfer, session/password
preservation, PostgreSQL HTTP startup/restart/shutdown, second-owner configuration
protection and unchanged SQLite source bytes. Production data was not used.

## Verification at the asynchronous-read checkpoint

On 2026-10-26, 372 ordinary Rust tests, all 13 PostgreSQL contracts, formatting,
all-target Clippy with warnings denied and diff checks passed. The asynchronous
baseline cache additionally proves that cancellation releases a waiting builder
and that completed frames remain shared. Debug and release real-server
HTTP/WebSocket integration passed, including credential revocation, keyed sends,
reconnect recovery, pagination, cross-device unread state and production ingress.
A fresh disposable PostgreSQL target passed real-binary migration, HTTP snapshot,
restart/shutdown, ownership/configuration and unchanged-source checks. No
production data was used. An isolated folder-lock failure in one parallel test
run did not reproduce in the isolated test or two subsequent full runs; startup
and transfer now distinguish actual lock contention from other file I/O errors.

At this checkpoint application mutations still used blocking entry points under
the engine mutation lock. The later mutation checkpoint below supersedes that
state for bot messages/reactions and read acknowledgements.

## Native application mutation checkpoint

On 2026-10-26, 373 ordinary Rust tests and all 15 PostgreSQL contracts passed,
with formatting, all-target Clippy with warnings denied and diff checks clean.
The PostgreSQL application regression blocks an HTTP append on the global cursor,
requires another conversation's reaction and read acknowledgement to complete,
then cancels the HTTP request. A metadata writer waits without holding the engine
guard until the accepted append commits/publishes. Re-enabling room membership
then replays its durable receipt without advancing the global cursor. A separate
native regression races two conversations at the receipt capacity limit and
requires exactly one commit, one capacity rejection and no extra sequence.
Conversation permit tests cover same-conversation ordering, independent progress,
metadata exclusion, cancelled acquisitions and bounded weak-entry cleanup.

The debug/release HTTP/WebSocket harness and real-binary PostgreSQL
migration/lifecycle smoke also passed at that checkpoint.

## Browser text mutation checkpoint

Plain browser message sends now use the same owned native append operation as
bot sends. Preparation preserves Send-only room permissions, trimmed Markdown
text, stable author IDs and filtered mentions. Attachments require Read before
loading any upload record. Revalidation rejects replaced accounts/conversations.
The accepted task retains admission and conversation ordering through commit,
memory publication and broadcast when its socket request is cancelled.
Attachment-free browser and bot sends skip the additional media-validation
engine phase.

The PostgreSQL application cancellation contract now runs through both browser
and bot ingress: a blocked append releases the engine guard, unrelated reaction
and cursor writes finish, metadata waits for publication, and a cancelled
transport still broadcasts its committed message. An ordinary browser regression
checks Send-only access, Markdown/mention publication, one notification, attachment
read denial and revoked Send permission without extra sequence allocation.

At this checkpoint, all 374 ordinary Rust tests, all 15 PostgreSQL contracts,
all-target Clippy with warnings denied, formatting and diff checks passed. The
ordinary suite requires loopback networking for storage mock servers; its
sandboxed run had seven socket-bind denials, and the permitted run passed fully.

Rebuilt debug and release binaries also passed the full HTTP/WebSocket harness,
including browser text sends, Markdown, replies/edits, revocation, reconnects,
pagination, cross-device unread state, folder migration and production ingress.
Browser slash commands (including `/tell`), administrative mutations, media flows
and outer bot command transaction control remain to be converted; this checkpoint
does not complete the full refactor.

## Native direct-message checkpoint

Browser `/tell` uses owned async message transactions for both existing and new
private conversations. Creation and the first message commit atomically; the
private directory, conversation index and metadata caches publish only after
commit. New conversations emit a scoped metadata notice, while existing ones emit
a conversation update. Shared metadata fencing keeps identities, permissions and
media reservations stable. Opposite-direction sends use the same stable-ID
conversation permit. Elevated `/sudo /tell` remains on the legacy command path.

All 375 ordinary Rust tests and all 15 PostgreSQL contracts passed, with all-target
Clippy (warnings denied), formatting and diff checks clean. The direct-message
regression injects a failed first insert, verifies directory/counter rollback and
no broadcast, races opposite-direction sends, checks creation-vs-send permissions,
and reopens the database to verify preserved identity/revision/history. The
PostgreSQL application regression additionally blocks a new private send, verifies
its directory stays unpublished, requires unrelated writes to finish, cancels its
transport, and checks commit/publication and its metadata notification.

## Native browser message-action checkpoint

Browser `/reply`, `/edit` and `/react` now perform retained-message lookup and
native writes outside the engine guard. A shared message operation owns media
reconciliation, revision checks and publication. The metadata fence and stable
account/conversation checks cover asynchronous phases. Selected-room actions and
global superuser edits keep their existing context rules. No-op edits skip writes
and notifications; reactions skip attachment reads; edits validate only added
attachment references and preserve original author, sequence and Markdown mode.

Native edit requests explicitly carry the editing principal for attachment checks.
This permits an authorized editor to attach their own unreferenced upload without
rewriting the original author's identity. Both backend implementations recheck
that principal under native media locks. Legacy command edits use the same typed
principal field until their outer command path is migrated.

All 377 ordinary Rust tests and 16 PostgreSQL contracts passed. The shared browser
action contract covers edit authority, editor-owned uploads, retained references,
identity/sequence preservation, no-op writes, reaction toggles, room/private reply
quotes and global edit context. The append-lock application contract additionally
requires native browser reaction and edit operations to finish in another
conversation before the append counter is released.

Remaining synchronous paths include `/retract`, `/clean`, elevated commands,
administrative/account mutations, media flows, outer bot command transactions and
console entry points. Their conversion and removal remain part of the full goal.

The rebuilt debug/release HTTP/WebSocket harness passed with native browser
sends, direct-message creation and reply/edit/react paths, including reconnects,
revocation, paging, unread state, migration and production ingress. An additional
regression expires a retained attachment, then verifies edits and reactions still
succeed without reloading or reauthorizing the unchanged attachment reference.

## Native history-deletion checkpoint

Browser `/retract` and `/clean` (including previews) now use native async lookup,
count and commit operations outside the engine guard. Deletions, reply-quote
scrubbing, attachment-reference changes, checked conversation revisions and audit
entries commit together. Audit writes append one entry and prune to 1,000 entries;
preparation no longer clones the audit ring. Memory and notifications publish only
after commit. No-op cleanup skips both database writes and audit publication.

Bulk cleanup locks conversation IDs in stable order before taking the shared
metadata fence, then revalidates the complete target set. This avoids a lock cycle
with queued metadata writers. An audit-only mutex covers audited deletion commit
and publication; independent sends, edits, reactions and cursors do not take it.
Legacy metadata writers still take the exclusive metadata fence before the engine.

Shared SQLite/PostgreSQL deletion contracts inject failures into retraction and
multi-conversation cleanup and require data, revisions, audit and notifications to
remain unchanged. They verify author checks, reply scrubbing, media reference
release, preview/no-op behavior, bulk denial, restart persistence and exact audit
cap/pruning. Separate tests cover stale native revision rejection and a bulk-lock
request concurrent with a queued metadata writer. PostgreSQL contracts require
unrelated retraction/cleanup to finish during a blocked append, and unrelated sends
to finish during a blocked cleanup. A cancelled cleanup must still commit, publish
its audit/notification and precede a queued metadata mutation without holding the
engine lock while waiting on PostgreSQL.

Elevated commands, administrative/account/query commands, media flows, outer bot
command transaction control and console entry points still require conversion.
The full database refactor remains in progress.

At this checkpoint, all 380 ordinary Rust tests and all 18 PostgreSQL contracts
passed, including the 1,000-entry audit-cap contract and cancelled cleanup. Formatting,
all-target Clippy with warnings denied, and diff checks passed.

Rebuilt debug and release binaries passed the full real-server HTTP/WebSocket
harness, including browser retraction, scoped permissions, unread/cursors,
reconnects, folder migration, account lifecycle and production ingress.

A fresh disposable PostgreSQL database also passed the real-binary provisioning,
verified offline migration, preserved-session/password, HTTP startup/restart/shutdown,
second-owner configuration protection and unchanged-source checks. Working data
folders were not used.

## WIP attachment-media checkpoint and resume handoff (2026-10-26)

This is a resumable checkpoint, not completion of the database refactor.

Attachment media reservation, ownership/read checks, completion, abandonment,
download revalidation and background cleanup now await typed native operations
outside engine guards. `engine/media_allocation.rs` prepares an owned reservation,
including a possible first private directory; quota checks and reservation/directory
insertion commit together before memory publication. Failed quota checks cannot
leak a directory. Reservations stay silent but invalidate metadata/routing caches
so the eventual first private message reaches existing subscribers. Existing
private uploads no longer issue an unnecessary directory upsert.

`engine/media_store.rs` owns the database handle and immutable media configuration
for asynchronous lifecycle operations. Ordinary media reads use the reader pool;
explicit command-scoped reads retain their connection and see its uncommitted
changes. Cleanup records confirmed object deletion and staging completion in one
native transaction while preserving the pending-URL grace period. Engine media
compatibility entry points with no production callers are test-only; the unused
synchronous staging-completion wrapper has been removed. Avatar storage helpers
still have production callers and must remain until avatar/profile migration.

`web/auth.rs::Identity` now supplies the shared stable browser identity used by
commands, cleanup and native media. HTTP admission performs memory-only credential
lookup as a capacity hint; each handler retains authentication/authorization and
storage-health responsibility. This removes a hidden database ping under the
admission middleware's engine read guard. API I/O/account leases remain owned by
accepted native work through cancellation. No general API lease is held across
external object-store probes, signing or finalization.

### Verified at this commit

- `rtk cargo test --locked`: 382 passed, 23 ignored.
- With a disposable `CHAT_TEST_POSTGRES_URL`,
  `rtk cargo test --locked postgres_ -- --ignored --test-threads=1`:
  all 20 PostgreSQL contracts passed. Run them serially.
- `rtk cargo clippy --all-targets --locked -- -D warnings`: passed.
- `rtk cargo fmt --all --check` and `rtk git diff --check`: passed.
- The latest SQLite/PostgreSQL HTTP media contracts use a loopback object-store
  mock. They cover private reservation, invalid hints, ownership, actual completion
  through HEAD/copy/range verification, info/download access, published-abandon
  protection, quota rollback, revocation, batch-delete fallback, staging grace and
  confirmed cleanup/tombstone removal.
- A PostgreSQL contract locks an uploader's quota row, requires another account's
  reservation and config request to complete, cancels the first HTTP request,
  queues revocation, and proves accepted commit/publication precedes revocation
  without holding the engine guard. Reservations remain silent.
- A SQLite WAL contract holds the single writer's open command transaction while
  an ordinary media read completes on a reader and sees committed state. The
  command-scoped media read sees its own uncommitted update; dropping the scope
  rolls it back.

The most recent full real-binary debug/release HTTP/WebSocket harness and
`auto/test-postgres.py` migration/lifecycle smoke passed at the preceding native
history-deletion checkpoint (380 ordinary tests / 18 PostgreSQL contracts). They
have **not** been rebuilt/rerun for the latest media/admission changes. A real S3
media smoke has also not been rerun for this media checkpoint. Do not present the
older real-binary results as verification of these latest edits.

### Remaining scope

- Convert avatar allocation, completion, visibility reads and profile/account
  metadata commits; `web/avatars.rs` still uses `media::authenticated` and legacy
  guarded engine operations. Attachment media is migrated; avatars are not.
- Convert administrative/account/credential operations and query commands that
  still reach storage through synchronous `Engine` methods.
- Convert `/sudo` command variants without retaining temporary elevated authority
  across an await; preserve command/action gates at each phase.
- Replace outer bot command transaction control and its synchronous nested
  savepoint/rollback bridge while retaining atomic effect-plus-receipt outcomes
  and authoritative memory recovery on failure.
- Convert console entry points, then remove obsolete production engine mutation
  paths and runtime adapters. Do not remove offline-transfer or test-injection
  capabilities needed by their actual callers.
- Rebuild debug/release binaries, rerun real HTTP/WebSocket integration and
  migration/lifecycle smoke against a fresh empty PostgreSQL target, and rerun
  relevant real media coverage after the remaining changes.
- Perform the full completion audit against the original typed async dispatcher,
  disjoint backend SQL, SQLite single-writer and PostgreSQL multi-writer goals.
  This commit deliberately leaves that goal unfinished.

### Resume map and invariants

- `engine/database/{dispatcher,context,transaction,runtime}.rs`: native pool
  dispatch, explicit command scope, root transaction lifetime and remaining bridge.
- `engine/database/{sqlite,postgres}/`: independent SQL/schema/lifecycle operations;
  shared storage must not regain SQL strings or dialect translation.
- `engine/{message_write,message_action,history_write,media_allocation,media_store,read_position}.rs`:
  owned operation preparation, native persistence and committed publication.
- `web/{mutations,commands,cleanup_commands,media,admission,auth}.rs` and
  `bots/mutations.rs`: application ordering, stable identity and accepted-work
  ownership. `web/media_tests.rs` and `database/dispatcher_tests.rs` carry the
  newest media/concurrency contracts.
- Acquire all sorted conversation locks before the shared metadata fence; legacy
  metadata writers acquire the exclusive fence before the engine lock. Never wait
  for that fence while already holding the engine lock.
- Keep the audit permit through audited commit/publication; ordinary message,
  cursor and media writes must not take that permit.
- The transactional append counter preserves global commit/cursor order. It is
  an intentional append serialization point, not a general PostgreSQL writer
  mutex. Metadata writes cannot overwrite it; publish only database-assigned
  sequences and committed revisions.
- Preserve SQLx rollback-on-cancellation, fail-closed handling of unknown commit
  acknowledgements/owner loss, a dedicated PostgreSQL ownership session, and
  asynchronous pool draining before ownership release.
- Keep v19 representation and lossless JSON/COPY transfer compatibility. Do not
  change schema versions merely for driver or application-path cleanup.
- Keep result caches above backend dispatch and validate snapshot/revision and
  credential state before delivery. SQLx owns prepared-statement caching.

### Disposable test environment at handoff

The local PostgreSQL 18.6 test cluster was verified running from
`/private/tmp/chatroom-sqlx-pg-81zz_gfu`, bound to `127.0.0.1:55439`, under the
scratch role `chat_refactor_test`. It may not survive a reboot: verify it rather
than assuming it is live or starting another server over its files. Contract tests
used the `postgres` database with isolated fixture schemas. The database
`chat_refactor_history_01a176e8` was filled by the last successful migration smoke;
that smoke requires a new empty target on the next run. Working data folders were
not used. Only stop/remove resources belonging to this disposable test cluster.
The ignored root `chat` build artifact is from the earlier history-deletion
checkpoint and must be rebuilt before treating it as the current source.

## WIP native avatar/profile checkpoint and resume handoff (2026-10-27)

This checkpoint supersedes the attachment-media handoff above for current scope
and verification. The full database refactor remains unfinished.

### Implemented

- Avatar allocation, completion, visibility, download and self-service avatar/color
  changes now await native typed SQLx operations outside engine guards. The unused
  blocking HTTP authentication helper is removed; legacy avatar/color/storage
  helpers without production callers are test-only.
- `engine/profile.rs` prepares an owned account delta and publishes only after
  commit. `database/profile.rs` dispatches an atomic account-plus-avatar operation
  to independent SQLite/PostgreSQL implementations. PostgreSQL locks the target
  account row; SQLite retains its single writer. No schema/version change occurs.
- `web/profiles.rs` holds per-account ordering and a shared metadata fence through
  preparation, commit and publication. Different accounts can progress separately.
  Accepted work retains admission after transport cancellation. No-op profile
  changes skip writes, change budgets and notifications. Only color updates rebuild
  color projections.
- `engine/avatar_store.rs` owns retry-safe reservations. PostgreSQL uses a
  transaction advisory lock on the retry ID before uploader quota locking, so two
  accounts cannot claim the same ID. Same-owner replay validates size, class and
  pending lifetime without allocating twice.
- Avatar reads batch historical-author visibility through native history readers,
  then revalidate identity and delivery stamps. Revocation can finish while a
  database history query waits; stale results cannot grant delivery.
- `web/avatar_tests.rs` covers atomic rollback, replay, cross-account retry races,
  historical visibility, restart persistence, account-local ordering, independent
  PostgreSQL progress and cancelled profile publication.

### Verification recorded during implementation

- Ordinary Rust suite: 384 passed, 27 ignored.
- Opt-in PostgreSQL contracts: all 24 passed serially against the disposable
  PostgreSQL 18.6 cluster described above.
- Formatting, diff checks and all-target Clippy with warnings denied passed.
- Debug and release binaries built successfully; the ignored root `chat` artifact
  was refreshed from this release build.
- Real RustFS media smoke passed, including avatar upload, owner checks, immutable
  bytes, restart, replacement/removal and attachment lifecycle/outage coverage.
- The debug/release HTTP/WebSocket harness and fresh-target PostgreSQL migration
  smoke were started, but their terminal results could not be recovered after the
  session boundary. Their completion is **unconfirmed** for this checkpoint; rerun
  them before claiming verification. The migration target
  `chat_refactor_profiles_01a176e8` may already contain data: use a fresh empty target.

### First unresolved issue on resume

Both `web/avatars.rs::complete` and `web/media.rs::complete` hold the S3 object's
`OwnedMutexGuard` in the outer HTTP future while awaiting a detached native
completion task (`web/profiles.rs::update` or `web/media.rs::mutate`). Cancelling
that HTTP future can release the object lock before database commit/publication.
A second completion could then finalize still-pending media again while the first
commit is waiting. This cancellation window is not fixed in this WIP.

Move the owned object guard into accepted completion work and retain it through
commit/publication. Preserve the rule that general API I/O/account leases are not
held across external S3 finalization. Add a regression that blocks the database
completion, cancels its HTTP request and proves another completion stays busy
until accepted work finishes. Cover both avatar and attachment completion, then
rerun relevant contracts and real media smoke.

### Remaining full-refactor scope

- Administrative/account/credential operations and storage-backed query commands.
- Elevated `/sudo` variants with authority revalidation across asynchronous phases.
- Outer bot command transactions/savepoints, atomic effect-plus-receipt handling
  and authoritative memory recovery on failure.
- Console entry points and removal of obsolete production synchronous adapters;
  retain required offline transfer and test injection capabilities.
- Final rebuilt HTTP/WebSocket and fresh-target PostgreSQL migration checks, then
  an audit against the original async dispatcher, disjoint backend SQL, SQLite
  single-writer and PostgreSQL multi-writer requirements. Preserve transactional
  append ordering, shared revision caches, ownership and cancellation invariants.

This work builds on `767e1b9` and deliberately preserves a resumable incomplete
state. Do not describe the driver conversion or this avatar/profile checkpoint as
completion of the application refactor.

## Native command-query and completion-lock checkpoint (2026-10-27)

The object-lock cancellation window recorded above is now fixed for attachment
and avatar completion. The accepted worker owns the lock through database commit
and publication. A PostgreSQL regression blocks each media completion update,
cancels its HTTP caller, requires a concurrent completion to remain busy, then
releases the update and verifies readiness, avatar publication and successful
idempotent retry. S3 finalization still runs outside general API admission leases.

`engine/command_read.rs` now owns history command parsing, authorization and output
formatting. `CommandRead` carries immediate text, a native history page plan or a
cleanup-preview plan. Legacy console/bot callers use compatibility completion;
browser read-only commands await native completion. The old production synchronous
history-page wrappers are test-only, and the duplicate blocking browser `/configs`
and read-command paths are removed.

`web/query_commands.rs` captures query plans and delivery stamps under short
guards, awaits history/health/cache statistics outside those guards, then checks
identity, authority and stamps again. It retries a changed captured state once.
Temporary `/sudo` authority exists only inside each synchronous capture; the shared
metadata fence is released before database reads. Revocation can therefore finish
while a history query is blocked, and stale output is rejected. Native `/configs`
uses `HistoryStore::stats_async`; synchronous statistics remain for console and
outer bot command paths until those are migrated.

Ordinary and PostgreSQL parity contracts compare history/range/private output,
memory-only query output, elevated previews and configuration output against the
existing domain behavior, preserving permissions and memory/revision state. The
PostgreSQL revocation contract covers ordinary read grants and elevated authority.

Verified after these changes: 385 ordinary Rust tests passed (30 ignored), all 27
opt-in PostgreSQL contracts passed serially, all-target Clippy with warnings denied
passed, and formatting/diff checks passed. Rebuilt real-server/media verification
is tracked below as it completes. The full refactor remains unfinished:
administrative/account/credential mutations, elevated writes, outer bot receipt/
savepoint control, console operations and final synchronous adapter removal remain.

Rebuilt debug/release HTTP/WebSocket integration, real RustFS media smoke and
fresh-target PostgreSQL migration/lifecycle smoke all passed at this query
checkpoint. The filled migration database is `chat_refactor_queries_01a176e8`.

## Native basic-administration checkpoint (2026-10-27)

Basic browser administration now uses owned native metadata transactions:
account enable/disable, grant/revoke (including group/bulk variants), room
creation/deletion, invitation/kick/join/leave and ownership transfer. Their
`/sudo` variants elevate only during synchronous preparation. The metadata fence
is acquired asynchronously and retained exclusively through commit/publication;
the engine guard is released during all database work. This exclusion stabilizes
authority and serializes global policy changes without occupying the engine lock.

`engine/metadata_write.rs::PendingMetadata` owns resolved-key before/after snapshots,
serialized typed operations, every new audit entry, compiled committed policy and
notification audiences. Preparation reuses the memory-only domain handlers and
restores the original state before returning, including on validation/serialization
failure. SQL remains in the independent backend operation modules. Commit failure
leaves memory/caches/notices unchanged. Publication applies only selected metadata,
retains the append counter and updates indices, colors, cursors and notices.
Committed publication always runs even when response delivery fails because the
request's session expired during I/O; self-revocation still publishes its effect.

`storage/entities.rs` separates owned entity/state deltas from borrowed audit
writing so accepted metadata plans need no reference to engine memory. Native
administration appends its new audit entries and relies on the backend's existing
1,000-entry pruning. A group assignment that first admits a participant records
both its admission and group audit entries. Legacy callers retain the shared delta
builder and audit persistence until their entry points are migrated.

`web/mutations.rs` adds an owned `MetadataPermit` alongside conversation permits;
both use the same short engine-phase implementation with coordinator validation.
Legacy `App::engine` still acquires that exclusive fence synchronously before the
engine guard. Do not call it from a worker already holding `MetadataPermit`.

Verified: 386 ordinary Rust tests (32 ignored), all 29 opt-in PostgreSQL contracts
serially, all-target Clippy with warnings denied, formatting and diff checks.
New shared SQLite/PostgreSQL contracts verify injected state-write failure rolls
back a newly inserted room, no memory/revision/color/notification leak, permissions,
membership/ownership, session revocation, room history/cursor removal, all audit
entries, append-counter preservation and restart persistence. A PostgreSQL contract
blocks the state row, observes old metadata and usable memory-only queries, cancels
the initiating HTTP work, then requires committed publication before a queued
metadata operation. Its second case expires the initiating credential while blocked
and still requires both committed publications and notifications.

Remaining full scope: account/room renames, account deletion, metadata/retention
object commands, account provisioning/password/auth lifecycle, bot management,
remaining elevated message/history mutations, outer bot command receipts/savepoints,
console entry points and removal of obsolete production synchronous adapters.
Offline transfer/test injection and original ordering/cache/ownership invariants
remain required. Real-binary verification for these latest administration changes
must be recorded separately from the preceding query checkpoint.

The rebuilt debug and release HTTP/WebSocket harness passed again for this
administration checkpoint. The current debug binary also passed the real RustFS
avatar/attachment smoke and the fresh-target PostgreSQL provisioning/verified
transfer/HTTP restart/shutdown/ownership/unchanged-source smoke. The filled target
is `chat_refactor_admin_01a176e8`; use another fresh empty target on the next run.
The ignored root `chat` executable was refreshed from the current release build.

## Native account commands and authentication checkpoint (2026-10-27)

Bulk `/user set` and metadata reset, room rename, password change, account
creation and password reset now use the native metadata path. Pure owned account
and room plans share validation with compatibility callers. Room rename commits
metadata, revisions and cursor-key movement together. Credential commands hash on
bounded workers without engine guards or metadata fencing, then recheck principal,
command/action grants, expected hash and reset target identity before persistence.
Accepted credential work retains admission and publishes after transport loss.
The synchronous password-change wrapper has no production callers and is test-only.

`engine/session_write.rs` owns native session deltas and authentication-event
persistence. Login/logout commit session insertion/eviction/removal and their event
in one transaction, then publish session memory and revision under a short guard.
The exclusive metadata permit stabilizes account/session state across the await.
Failed-login events use their own native write. Login attempt reservations and
password permits remain owned by accepted work through completion/cancellation;
login continues to use no general HTTP I/O permit. Compatibility login/logout and
authentication-event/session save methods are now test-only.

Browser socket upgrade/reauthentication and session-status checks await database
health outside engine guards and revalidate stable credential identity afterward.
Presence changes perform memory checks only. The former guarded mutable API helper
and synchronous browser authentication helper now have test-only callers.

Shared SQLite/PostgreSQL application contracts cover bulk metadata failure after
an earlier account update, scope/quota/color rules, room/cursor rename rollback,
password/session rollback and credential replacement. PostgreSQL cancellation
contracts cover password reset followed by queued revocation and login blocked on
its event write followed by queued disable; unrelated session reads remain usable.
Native auth contracts inject event failure into both login and logout and require
session memory, revision and notifications to remain unchanged. The ordinary
failed-login/audit-field/throttling and sudo regressions still pass.

Latest verification: 387 ordinary Rust tests passed (35 ignored), all 32 opt-in
PostgreSQL contracts passed serially, Clippy with warnings denied passed, and
formatting/diff checks passed. Debug/release HTTP/WebSocket integration passed for
the preceding native account-command changes; the latest session/socket changes
are being rebuilt and must have their real-binary result recorded independently.
The disposable PostgreSQL cluster was confirmed stopped and restarted from its
existing scratch directory on port 55439; do not assume its old PID is still live.

Remaining: account rename/deletion, bot management and read/control entry points,
remaining elevated message/history writes, outer bot command receipts/savepoints,
console migration, startup/maintenance bridge audit and obsolete production adapter
removal. The complete original goal is still active and unfinished.

The current native session/socket debug and release builds passed the full
HTTP/WebSocket harness. Real RustFS avatar/attachment smoke and the fresh-target
PostgreSQL migration/lifecycle smoke also passed. That smoke filled
`chat_refactor_auth_01a176e8`; use a new empty target next time. The ignored root
`chat` artifact now matches this release build. No source changes followed these
builds other than documentation and test-fixture permission diagnostics.

## Native account identity and slash-bot administration checkpoint (2026-10-27)

Account rename/deletion now use native transactions. Rename produces an authorized
`AccountRenameRead`, awaits indexed history/reaction references on a reader outside
the engine guard, then prepares its owned delta under the same metadata permit.
Stable target ID and actor authority are rechecked before preparation. Sessions,
directory/membership keys, affected revisions, history/reaction/mention/reply
rewrites, name-based cursors and audit commit together. Deletion similarly commits
anonymization, private-chat removal, session/receipt/cursor cleanup and bot ownership
recovery before publication. `PendingMetadata` applies the corresponding cursor/
account effects only after commit. Reusing a deleted username never reuses its ID,
credentials, history ownership or upload ownership. Compatibility entry points
share the memory mutation and validation logic with these native paths.

Slash `/bot` creation, owner assignment, credential issuance/revocation and removal
also use owned native metadata deltas. Read-only bot listing/info/credential listing
use the async query path and do not reveal credential secrets. The versioned bot
HTTP control/command-receipt paths remain separate unfinished work; do not infer
their completion from the slash-command migration.

Verified: 388 ordinary Rust tests passed (37 ignored), all 34 PostgreSQL contracts
passed serially, all-target Clippy with warnings denied and formatting/diff checks
passed. Current debug/release builds passed the full HTTP/WebSocket harness,
including account deletion, username reuse and bot operations. Shared identity
contracts inject failure after cursor/history work and require full DB/memory/
notification rollback, then verify sessions, reactions, reply/mention names, private
and room cursors, unaffected revisions, retained uploads, bot ownership and restart.
PostgreSQL cancellation contracts block rename reference lookup and deletion cursor
cleanup while ordinary authorized queries remain usable, cancel the HTTP caller,
and require publication before queued disable or username replacement.

Remaining original scope: versioned bot control/read/credential paths and outer
command receipt/savepoint transactions; elevated message/history writes; console
migration; startup/maintenance bridge audit; obsolete production adapter removal;
and final full-scope verification. The goal is not complete at this checkpoint.


## Native versioned bot control checkpoint (2026-10-27)

Versioned bot credential issuance and conversation creation now prepare owned
metadata deltas, await native writes outside engine guards, then publish under the
same exclusive metadata permit. Existing private conversations use a native health
check and return without a spurious metadata notice. Accepted tasks retain their
admission permits through cancellation and commit/publication. Pure credential
validation/mutation is shared with test compatibility callers; the obsolete
synchronous empty-private creation wrapper has been removed.

Bot source-message facts and command receipt lookup await native reader operations,
then recheck live credential scope, target revision and author identity before
returning. Receipt reads now use reader leases unless already inside an explicit
command transaction. Bot command/schema discovery is memory-only after native
health/authentication. Socket upgrade, heartbeat, activity and control admission
also await health outside guards; catalogue registration, refresh and responses
use short memory-only capture. Browser bot-command discovery uses stable session
identity with the same native health/revalidation path. The former browser
`engine_read_api` bridge now has only test callers.

Shared SQLite/PostgreSQL contracts inject failure into credential metadata updates
and private-directory insertion, require unchanged DB/memory/revision/notices,
then exercise successful scoped credentials, repeated private creation and live
catalogue credential revocation. PostgreSQL additionally blocks source lookup and
revokes the bot while readers leave the engine available, and cancels HTTP
credential issuance while its state update is locked: accepted work must commit
and publish before queued disable. Current-source verification passed all 389
ordinary Rust tests (40 ignored), all 37 opt-in PostgreSQL contracts serially,
all-target Clippy with warnings denied, formatting and diff checks. Debug/release
builds succeeded. The real HTTP/WebSocket harness result is recorded below when
complete.

Remaining original scope: outer bot command effect/receipt transactions and nested
savepoint removal; remaining elevated message/history writes; console migration;
startup/maintenance bridge audit; obsolete production adapter removal; final full
verification. The complete goal remains active.

The current native bot-control debug/release binaries passed the full real
HTTP/WebSocket integration harness, including durable keyed sends, live bot
worker discovery/reconnect, scoped credentials/revocation and production routing.
The ignored root `chat` executable matches this release build. No current-turn
S3 or fresh-target transfer smoke was run: the preceding identity/auth checkpoints
record their most recent results. Those checks remain part of final verification.


## Native elevated message/history writes checkpoint (2026-10-27)

Browser `/sudo /tell`, `/reply`, `/edit`, `/react`, `/retract` and `/clean` now
route through native owned plans and asynchronous reads/writes. Shared capture
and phase helpers validate the stable session and install temporary command
authority only for synchronous planning. Ordinary initial captures retain their
read guard path; elevated initial captures take a short mutable guard under an
empty shared metadata permit, then release that permit before acquiring sorted
conversation locks. Every later preparation/media-validation phase checks sudo
again, with no temporary authority surviving a database await. Publication never
requires elevation and still runs after cancellation of accepted writes.

Shared SQLite/PostgreSQL contracts cover denied ordinary writes, elevated replies,
private sends, cross-room edits/retractions, reactions and cleanup, stable author
identity/reply quotes, audit entries and unchanged authorization. Injected message
insertion failure must roll back sequence, revisions, memory, history and notices.
PostgreSQL contracts block the initial unscoped message lookup, revoke sudo and
require rejection after lookup resumes. Separate cancellation cases block an
accepted reply or cleanup, cancel the caller, and require commit/publication before
queued sudo revocation while the engine remains readable and unelevated. Restart
checks require persisted cleanup/private history and no elevated account state.

Remaining complete scope: native outer bot command effects plus atomic outcome
receipts/savepoint removal; console operations; fallback command errors;
startup/maintenance bridge audit; obsolete production adapter removal; final
full-scope verification. The original goal remains active and incomplete.

Current-source verification: 390 ordinary Rust tests passed (43 ignored), all 40
opt-in PostgreSQL contracts passed serially, all-target Clippy with warnings denied,
formatting and diff checks passed. The shared elevated-write contract also exercises
referenced attachments: media records are read without sudo/engine guards, then
media authorization is rechecked in a fresh synchronous sudo phase. Debug/release
builds succeeded; their integration harness result is recorded below when complete.


Cleanup previews (`--dry-run`) route directly through the owned read-only query
path, including sudo previews. They no longer acquire conversation mutation
permits or retain the metadata fence during counting. The query path revalidates
authority and delivery stamps after native reader work. Final current-source
checks after this routing adjustment must be recorded independently below.

Final routing adjustment passed 390 ordinary Rust tests, all 40 PostgreSQL
contracts, Clippy with warnings denied, formatting and diff checks. Current debug
and release builds succeeded; the earlier integration harness passed before the
preview routing adjustment, and its final rerun is in progress. Real RustFS media
smoke passed against the current debug build, including attachments, avatars,
restart, storage outage rejection and recovery. No fresh-target PostgreSQL
migration/lifecycle smoke was rerun in this checkpoint.

The final current-source debug/release HTTP/WebSocket integration harness passed.
The ignored root `chat` executable matches this release build. The elevated native
write and read-only cleanup-preview changes are verified; the full refactor remains
unfinished in the outer bot command transaction, console/fallback paths and final
bridge removal/audit described above.
