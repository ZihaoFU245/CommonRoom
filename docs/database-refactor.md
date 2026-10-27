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
