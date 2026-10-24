# Backend review implementation

This is the implementation and verification ledger for the 2026-10-22
`analysis.md` review. A finding is complete only when its behavior and relevant
regressions are verified. Existing security/authorization invariants remain required.

| Finding | Status | Work / verification |
| --- | --- | --- |
| CQ-1 | Complete | Separate Store, Presence, CompiledPolicy and media owners; production Data is private and transport uses narrow identity/credential/history methods. Account lifecycle and independent media-policy regressions pass. |
| CQ-2 | Complete | Engine domain, authorization, command, storage, history and snapshot failures retain typed classes; centralized browser/bot mapping; SQL rollback/privacy/hidden-resource regressions pass. Text formatting remains only at legacy command/console boundaries. |
| CQ-3 | Complete | Shared socket lifecycle, heartbeat, idle timeout and frame budgets. |
| CQ-4 | Complete | Explicit resolved keys replace pointer comparisons; selected-row delta/rollback tests pass. |
| CQ-5 | Complete | Removed full-rewrite fallback, reaction reconciliation and obsolete save test helper. |
| CQ-6 | Complete | Strict config keys; top-level/nested rejection preserves original file. |
| CQ-7 | Complete | Skip unchanged values; exclusive temporary file, file/directory sync and rename. |
| CQ-8 | Complete | Central message/command/password/reaction limits; Unicode boundary regressions and documented units. |
| CQ-9 | Complete | Password/provisioning module; one blocking command worker; off-lock hashing and commit-time authority checks; engine reports committed change kind. |
| CQ-10 | Complete | Thin snapshot assembler composes explicit authorization, visible-conversation and directory helpers; owned plans load tails/unreads and assemble outside the engine guard. |
| CQ-11 | Complete | Moved large suites into explicit test directories; replaced obsolete pointer/save assertions. |
| CQ-12 | Complete | Explicit shutdown event and shared cancellation independent of heartbeat. |
| CQ-13 | Complete | Shared message target/scope resolution preserves context and su-only global room lookup. Distinct scope parsers and independent authorization boundaries retained. |
| SEC-1 | Complete | Fail-closed malformed forwarding, bounded IP/account failure admission. |
| SEC-2 | Complete | Exponential account backoff and bounded failure logging. |
| SEC-3 | Complete | Four sockets per account/profile; oldest replacement; separate browser/bot pools. |
| SEC-4 | Complete | Eight HTTP jobs per stable account; cancellation retains worker leases; independent login. |
| SEC-5 | Complete | Committed metadata audiences target stable accounts using old/new rights and changed scopes; bot credentials send compact catalogue invalidations to other readers. Coalescing unions audiences and preserves discovery during recovery. New/revoked membership, unrelated-viewer history counters, cold-snapshot comparisons, real sockets, browser and media regressions pass. |
| SEC-6 | Complete | Bot socket controls share the HTTP per-account input budget. |
| SEC-7 | Complete | Digest session keys; atomic baseline v17 migration, collision/rollback/restart tests; host-prefixed root cookies and duplicate-cookie rejection. |
| SEC-8 | Complete | External avatars denied by default; exact canonical host allowlist; stored URL filtering; managed-image caps preserved. |
| SEC-9 | Complete | Security headers on both accepted and rejected production ingress requests. |
| SEC-10 | Complete | Direct bot HTTP requires opt-in and private/loopback trust; forwarded HTTP always rejected; deployment contract and tests. |
| SEC-11 | Complete | Bounded durable authentication events; verified outcomes and valid logouts; atomic session/event persistence and failure injection. |
| INE-1 | Complete | Owned browser/bot snapshot plans; SQLite, assembly and serialization outside engine guards; live identity/revision revalidation and bounded retries; blocked-reader revocation tests. |
| INE-2 | Complete | Existing stable-ID index now routes browser updates; rename/deletion/private-access regression. |
| INE-3 | Complete | Exact incremental append/cursor deltas proven by row/revision counts; fallback after retention, edits and cleanup; gapped-sequence SQL oracle and failure regression. |
| INE-4 | Complete | Transactional retained-row counters; prune only excess rows, without walking retained OFFSET; migration, gaps, limits, cleanup and rollback tests. |
| INE-5 | Complete | Metadata epoch and bounded browser/bot templates; complete frames keyed by exact visible history/read/presence stamps; cross-viewer history pages shared; temporary sudo bypasses persisted templates. |
| INE-6 | Complete | At most eight notifications share one blocking worker; browser owned conversation/unread/avatar plans and final live stamps; one-guard compact bot batches; bounded negative routing hints with atomic metadata invalidation; mixed-batch, revocation, scope and real private-reservation regressions pass. |
| INE-7 | Complete | Four conversation-hashed cache shards share the original global budgets; independent-lock and poison/bounds tests. |
| INE-8 | Complete | Resolved entity keys flow from account, room, policy, bot, rename and deletion paths into persistence/rollback; no full Data clone in bot administration; targeted selection and failure tests. |
| INE-9 | Complete | Prune only after append, seeking the first retained audit ID instead of OFFSET. |
| INE-10 | Complete | One-second failed-probe backoff and download tickets; sign once, recheck identity/access/expiry and SQL write stamp. |
| INE-11 | Complete | Presence owns connection counts and shared active-directory/frame projection; metadata/connection invalidation and coalesced updates; shared-frame and multi-tab tests. |
| INE-12 | Complete | One blocking login worker; password admission before bounded bookkeeping. |

## Required validation

- Focused permission, resource isolation, persistence/rollback, expiry and race tests.
- `auto/check.sh` after each coherent implementation stage.
- HTTP/WebSocket integration against rebuilt debug and release binaries.
- Real browser checks against `web/`, with isolated data, at desktop and compact widths.
- Media transport tests and local RustFS smoke when changing media behavior.
- Final finding-by-finding audit against current code, tests and this ledger.

The initial committed pass (`0710138`) passed 291 Rust tests, 265 web tests,
repository checks, the HTTP/WebSocket harness, five browser regressions and a
real-application browser scenario. Later stages must rerun checks for changed behavior.

## Second stage verification

- `auto/check.sh`: 303 Rust tests passed (3 opt-in ignored), 295 bot-worker
  tests passed (2 ignored), 265 web tests passed, source/format/lint checks clean.
- Rebuilt debug/release binaries passed the complete HTTP/WebSocket harness.
- Added real-socket pressure checks for oldest replacement, pool/account
  isolation, shared socket/HTTP bot budget, and shutdown within three seconds.
- Real Chrome UI passed invalid password, account backoff, socket startup, Unicode
  message send, reload persistence, compact layout, repeated successful login
  and logout with no uncaught exceptions.
- Replaced the stale prepared-send conflict assertion with deterministic receipt
  replay, retaining separate unrelated-revision and revoked-permission denials.

## Final implementation status

All 36 findings are complete. The final requirement audit and current validation
results are recorded below. Earlier stage summaries describe their status at that time.

Database support now accepts only the current HEAD schema, v19, and initializes
empty databases transactionally. The earlier migration findings below record
historical work; their migration code and upgrade-success fixtures have since
been removed. Unsupported-version rejection, digest-based sessions and current
persistence/rollback tests remain.

Review correction: `read_command` is an independently callable authorization
boundary. `execute_inner` returns immediately when its read fast path matches,
so it does not then execute `apply`. Removing its command check as suggested in
CQ-13 would weaken security. Preserve that guard while consolidating actual
parser/target duplication. Likewise, the preexisting media download path already
signed once; its second live authorization check was required. Download tickets
optimize the repeated metadata lookup while retaining that final check.

## Third stage verification

- `auto/check.sh`: 309 Rust tests passed (3 opt-in ignored), 295 bot-worker
  tests passed (2 ignored), 265 web tests passed, formatting/lint/type checks clean.
- Latest debug/release binaries passed the full HTTP/WebSocket harness, including
  explicit direct-bot HTTP opt-in and account resource-pressure checks.
- Real isolated RustFS smoke passed upload/completion, private reads, avatars,
  retraction, restart, startup cleanup, outage rejection and delayed recovery.
  The obsolete immediate-recovery assumption now waits out the documented
  one-second negative probe cache; pending-slot preservation remains checked.
- The real Chrome application scenario passed again with no uncaught exceptions.
- Configuration round trips preserve avatar hosts and direct-bot policy; invalid
  wildcard hosts/public direct-bot trust leave saved files unchanged.

The goal remains active. Open findings in the table above still require code,
regressions and final verification; passing these stages is not full completion.

## Fourth through seventh stage verification

- Session/audit/retention stage: full repository check passed 312 Rust, 295
  bot-worker and 265 web tests; rebuilt debug/release transport smoke, real Chrome
  login/message/reload/compact/logout checks and isolated RustFS media smoke passed.
- Incremental unreads, command-worker separation, owned browser/bot snapshot plans,
  shared presence and bounded coalescing: full repository check passed 317 Rust,
  295 bot-worker and 265 web tests. Three opt-in Rust and two bot tests remain ignored.
- Deterministic blocked-reader tests prove engine writers progress while snapshots
  wait, and both browser and bot frames discard revoked access or credentials.
- Latest message-target/account-notification/avatar-idempotency changes passed the
  317-test Rust suite. The final repository check passed again, followed by rebuilt
  debug/release transport smoke, isolated RustFS media smoke and the real Chrome
  desktop/compact login/message/reload/logout scenario, with no uncaught browser errors.
- Typed statuses replaced outdated smoke expectations: denied allocation/read is
  forbidden, absent private conversations and concealed upload ownership are not found.

## Metadata cache and resolved-key stage

- Metadata capture now composes authorization, visible-conversation and directory
  helpers. Browser/bot templates separate metadata from message revisions and
  viewer cursors; shared serialized frames survive unrelated history and sessions.
- New regressions compare reused projections with cold builds, verify revocation,
  rename/private/profile/recovery invalidation, bound template allocations/profiles,
  and prevent temporary sudo projections entering persisted authority caches.
- Every production administrative commit carries resolved account/session/room/
  private/bot keys. Rollback restores only those keys, including missing destination
  names. Bot administration no longer clones Data. Renames expand through indexed
  references; deletion explicitly includes the conversation revisions it changes.
- Full repository validation: 325 Rust, 295 bot-worker and 265 web tests passed;
  formatting, lint and type checks clean. Final rebuilt debug/release HTTP/WebSocket
  smoke, isolated RustFS media smoke and real Chrome desktop/compact checks all passed.
  Login/backoff, messaging, reload persistence, logout and uncaught-error checks passed.
- New files are intent-to-add for review. Git now recognizes all ten test-file
  relocations as renames; no file contents are staged and no commit was created.

One finding remains: SEC-5 (general metadata invalidation). The complete goal remains active.

## Ownership boundary stage

- Store owns writer/readers, exclusive folder lock, read cursors and pending history
  maintenance. Engine coordinates separate storage, presence, compiled policy and
  media owners. Media policy can be tested without an Engine.
- Production Data is private; web and bot adapters use narrow account ID/name,
  password hash and credential/owner lookups. Login copies only verification material.
  Source-author lookup uses the existing stable ID index instead of scanning accounts.
- Account lifecycle regressions cover disabled identities, re-enable, rename, deletion
  and same-name replacement. Full repository checks passed 327 Rust, 295 bot-worker
  and 265 web tests; the existing opt-in ignores remain unchanged.
- Real Chrome desktop/compact login, backoff, message send, reload persistence and
  logout checks passed with no uncaught browser errors.
- Rebuilt debug/release HTTP/WebSocket and bot integration smoke passed, including
  revocation, same-name account replacement, unread synchronization and restart.
  Isolated local RustFS smoke passed uploads, avatars, private reads, cleanup,
  restart, outage rejection and recovery. `git diff --check` is clean.

## Typed failure boundary stage

- Storage, history, authorization, domain commands, sudo and snapshot cache paths
  preserve EngineError through rollback and revalidation. No engine operation uses
  String as its error type. Argument parsers select Invalid explicitly; persistence
  failures select sanitized Unavailable at source.
- Browser history/snapshot/catalogue HTTP paths and native bot commands project
  classes centrally. Native commands distinguish invalid input, denied authority,
  conflicting state and failed storage; frozen v1 maps internal errors to unavailable.
  Browser command frames and console output retain their final text presentation.
- New native-command HTTP regression checks 400/403/409/503, retryability, private
  diagnostic suppression, rollback and recovery. Existing hidden-policy equality
  regressions compare both class and message; catalogue absence/access stays concealed.
- Full repository checks passed 328 Rust, 295 bot-worker and 265 web tests. A final
  Rust rerun and strict Clippy passed after the last snapshot projection changes.
  Real Chrome desktop/compact login, backoff, messaging, reload and logout passed
  without uncaught errors. The original three Rust/two bot opt-in ignores remain.
- Final rebuilt debug/release HTTP/WebSocket and bot integration smoke passed.
  Isolated local RustFS media smoke passed, including outage rejection/recovery,
  private reads, avatar lifecycle and restart. `git diff --check` is clean.

## Targeted avatar stage

- Actual self-service/managed avatar changes use stable-account invalidations and
  compact browser profile frames. No message tail or unread query is built. Bots
  skip avatar events; full baselines safely subsume queued profile invalidations.
- Owned visibility plans preserve directory/member/historical-author access. One
  indexed history query runs outside the engine guard. Deterministic blocked-reader
  tests prove writers can revoke access or sessions before frame delivery; revoked
  profiles emit no frame. Existing page/unread cache miss counters stay unchanged.
- The independent four-attempt/ten-second account budget is capped at 4096 tracked
  accounts. Tests cover account isolation, expiry, exhaustion, unchanged/repeated
  values and retained committed state. Client tests cover replacement, removal,
  malformed frames, prototype-safe names and preserved history references.
- Full repository checks passed 332 Rust, 295 bot-worker and 267 web tests, with the
  original opt-in ignores unchanged. The updated transport coalescing suite passed.
- Real Chrome applied avatar update/removal in the UI without receiving a full
  snapshot, then passed login/backoff, messaging, reload, compact layout and logout.
  Rebuilt debug/release transport smoke and isolated RustFS media smoke passed;
  managed-avatar completion explicitly verified compact delivery without a baseline.
- SEC-5 remains in progress for general metadata invalidation. INE-6 still needs
  queued browser/bot changes rendered in one blocking worker with batched live
  authorization; owned conversation/unread plans must keep SQL outside engine guards.

## Batched live delivery stage

- Browser and bot transports render up to eight queued notifications in one
  blocking worker. Coalescing still drains at most 64 input events and preserves
  discovery, presence, lag recovery and shutdown. Unread updates coalesce by viewer.
- Browser conversation/unread/avatar plans release the engine guard for SQL and
  serialization. A final live identity and relevant metadata/content/read/presence
  stamp revalidates the whole batch. Compact bot events share one credential/scope
  capture; mixed full-baseline/presence delivery also revalidates every frame.
- Conversation/discovery/activity routing uses at most 4096 IDs per socket. These
  are negative hints only; stale positive hints cannot grant access. Oversized
  projections use live routing. A read-only atomic metadata clock invalidates stale
  exclusions immediately, including silent private media reservations.
- New/expanded regressions cover mixed targets, scoped/expired/revoked credentials,
  baseline subsumption, bounded chunks, cache invalidation/deletion and blocked SQL.
  Writers revoke room/directory access or sessions while reads wait, and no stale
  conversation, unread, baseline or presence frame survives final validation.
- Final repository checks passed 337 Rust, 295 bot-worker and 267 web tests, with
  the original opt-in ignores unchanged. Rebuilt debug/release HTTP/WebSocket/bot
  smoke passed. Isolated RustFS smoke verifies first private delivery after silent
  reservation, compact avatar delivery, private reads, restart, cleanup and outages.
  Real Chrome passed avatar update/removal without baselines, login/backoff, messages,
  reload, compact layout and logout with no uncaught errors. Diff checks are clean.
- INE-6 is complete using batched live validation rather than postponing credential
  revocation until the heartbeat. SEC-5 remains open for general metadata targeting,
  followed by the final finding-by-finding completion audit.


## Targeted metadata stage and final requirement audit

Final validation on the current source completed successfully:

- `auto/check.sh`: 342 Rust tests passed, 3 opt-in tests ignored; 295 bot-worker
  tests passed, 2 ignored; 267 web tests passed. Formatting, Clippy with warnings
  denied, TypeScript, ESLint, syntax and shell checks passed.
- Rebuilt debug and release binaries passed the complete HTTP/WebSocket/bot harness.
  Hidden-room creation, grant, kick and regrant update affected viewers while an
  unrelated connected viewer receives no new baseline. Revoked room unread state
  disappears before regrant.
- Real Chrome against `web/` passed desktop and compact layout, invalid-password
  and backoff, message send/reload persistence, twelve successful logins, logout,
  and avatar update/removal without a new baseline. No uncaught browser exceptions.
- Isolated local RustFS passed media upload/completion, idempotency, private reads,
  first private delivery after silent reservation, avatar lifecycle, restart,
  cleanup, outage rejection and recovery. Real `data/` was not used.
- `git diff --check` passed. The three Rust opt-ins are two manual benchmarks and
  the provider integration fixture; local RustFS behavior was exercised through
  the separate media harness. No numerical overall speedup is claimed.

The final audit uses the original numbered findings in `analysis.md`, including
its verified non-issues. The following source boundaries and regressions provide
per-finding evidence; test names refer to the current 342-test Rust run above.

| Requirement | Current implementation and verification evidence |
| --- | --- |
| CQ-1 | `engine/mod.rs` keeps production Data private; `storage.rs::Store`, `presence.rs::Presence`, `authorization.rs::CompiledPolicy` and `media.rs::State` own their respective state. `transport_identity_lookups_follow_disable_rename_and_replacement` verifies adapter identity lifecycle. |
| CQ-2 | `engine/error.rs` retains six domain classes, and `web/error.rs` plus `bots/error.rs` project them centrally. `media_and_avatar_storage_failures_are_retryable_instead_of_invalid_requests`, `typed_errors_distinguish_permissions_missing_records_and_failed_storage`, and poisoned-history tests verify statuses and failure isolation. |
| CQ-3 | Both socket adapters call `web/transport.rs` for lifecycle, heartbeat, idle timeout and frame budget; transport budget/shutdown tests and real socket pressure checks pass. |
| CQ-4 | `changes.rs::DirtyKeys` and explicit optional `DataView` families select persistence; `storage.rs::write_map_keys` never uses pointer equality. Selected-row/rollback tests verify missing inserts and unrelated rows. |
| CQ-5 | No `write_data`, full-rewrite fallback or test `save()` remains; test persistence uses captured deltas. Hot transaction failure and current-schema restart tests remain. |
| CQ-6 | Config structs reject unknown fields before persistence; `unknown_configuration_fields_fail_without_rewriting` covers top-level and nested typos. |
| CQ-7 | `config.rs::persist_config` uses exclusive sibling creation, file sync, rename and Unix directory sync; unchanged normalized values skip persistence. File identity/format preservation tests pass. |
| CQ-8 | `limits.rs` centralizes message/command/password/reaction limits and documents Unicode versus byte units. `unicode_messages_and_commands_have_distinct_consistent_boundaries` and engine Unicode tests verify boundaries. |
| CQ-9 | `web/commands.rs` delegates hashing/provisioning to `credential_commands.rs` within one blocking job; `execute_command` returns its committed change. Password authority/session/target race tests pass. |
| CQ-10 | `queries.rs` composes scope rights, visible conversations and directory helpers; owned `snapshot.rs` plans handle tails/unreads/assembly. Cold-cache comparisons and blocked-reader tests verify projections. |
| CQ-11 | Ten large test files moved under explicit `tests/` module paths; Git recognizes the relocations. Tests retain private access without entering production compilation. |
| CQ-12 | Main sends explicit Shutdown; shared transport cancellation wakes idle subscribers independently of heartbeat. Shutdown unit tests and real socket shutdown-under-three-seconds checks pass. |
| CQ-13 | Shared message target/scope resolution removes repeated conversions. Independent `read_command` authorization and semantically distinct scope parsers are deliberately retained; scoped command, sudo and private-action denial tests pass. |
| SEC-1 | Trusted forwarding accepts one valid IP only, with explicit disabled-forwarding support. IP/account windows are bounded and expire incrementally. Spoofing, malformed/duplicate forwarding and saturation tests pass. |
| SEC-2 | Account failures apply 1–32 second backoff across client addresses and bounded failure logging; successful/unavailable attempts refund reservations. Backoff and real browser 429 tests pass. |
| SEC-3 | Admission owns four sockets per stable account/profile, replaces the oldest, and separates browser/bot global pools. Isolation/replacement tests and real socket pressure pass. |
| SEC-4 | Admission caps eight HTTP jobs per stable account; blocking work retains its lease through request cancellation. Account isolation/cancellation tests pass; login uses its independent password pool. |
| SEC-5 | `notices.rs` derives committed stable-account audiences from resolved changes and old/new authorization. Outsider history counters stay unchanged, grant/revocation frames match cold snapshots, and socket coalescing unions recipients. Bot credential issuance also broadcasts catalogue invalidation; API and real socket regressions pass. |
| SEC-6 | Native socket controls and bot HTTP share `admit_bot_account`; multi-credential/multi-socket budget tests and real pressure checks pass. |
| SEC-7 | Session lookup uses a domain-separated digest; v17 migration replaces keys atomically. Root production cookies use `__Host-`, scoped deployments retain their path cookie, and duplicate cookies fail. Migration collision/rollback/restart and production cookie tests pass. |
| SEC-8 | External avatars require an exact canonical allowlisted host; stored disallowed URLs are filtered and managed image caps remain. Host-policy and real avatar lifecycle tests pass. |
| SEC-9 | Ingress applies common security headers after both rejection and acceptance paths. `production_ingress_rejections_always_have_security_headers` verifies early responses. |
| SEC-10 | Direct bot HTTP requires explicit opt-in plus private/loopback trust; explicit forwarded HTTP remains rejected. Production bot ingress unit/integration tests pass. |
| SEC-11 | Separate durable auth events retain bounded verified success/failure/logout outcomes without credentials. Atomic session/event failure injection, restart and real login/logout tests pass. |
| INE-1 | Browser/native bot snapshots capture owned plans, then release engine guards before SQLite and serialization, and revalidate live credentials/revisions. Blocked-reader tests prove writers progress and revoked frames are discarded. |
| INE-2 | Conversation updates use the stable-ID index; indexed rename, deletion, private-access and same-name replacement regressions pass. |
| INE-3 | Unread cache proves exact append/cursor deltas from revisions and row counts, falling back after retention or mutation. The gapped-sequence SQL oracle covers edits, cleanup and failures. |
| INE-4 | Transactional retention counters prune only excess rows without retained-window OFFSET traversal. Gap, limit, cleanup, failed append and migration tests pass. |
| INE-5 | Bounded metadata templates and complete serialized frames use metadata/history/read/presence stamps; message/session churn reuses metadata and pages share payloads. Cold comparisons, cross-credential isolation, sudo bypass and cache bounds tests pass. |
| INE-6 | At most eight notifications share one worker; bounded coalescing and negative routing hints skip unrelated events, with atomic metadata clock invalidation. Mixed batches, blocked-read revocation and real first-private-message-after-reservation tests pass. |
| INE-7 | History and unread caches have four conversation-hashed shards with quartered global budgets. Independent-lock and per-shard LRU/byte/page bounds tests pass. |
| INE-8 | Administrative changes capture resolved keys and omit message bodies; bot administration does not clone Data. Selection, indexed references and keyed rollback tests pass. |
| INE-9 | Audit storage appends only new entries and prunes by the first retained unique ID only after an append. Row/revision-count and audit rollback tests pass. |
| INE-10 | Failed S3 probes share one-second backoff; downloads sign once and validate an owned ticket against live access and storage stamps. Probe recovery, ticket revocation and local media outage/recovery tests pass. |
| INE-11 | Presence owns connection counts and shared active-directory/frame projections; extra tabs cause no visible presence change. Projection-sharing tests and real multi-tab socket checks pass. |
| INE-12 | Login acquires password admission before bounded attempts bookkeeping and runs one blocking job; it never consumes general HTTP workers. Busy-password and repeated successful login tests pass. |

Metadata audiences are intentionally conservative for su-only generic avatar
metadata edits, because retained historical authors can be visible. The public
self-service avatar path retains compact, bounded profile invalidations. Directory
identity changes still refresh everyone authorized to see that directory. Neither
case restores the ordinary authenticated full-broadcast amplification.

The final audit also preserves the original non-issues: parameterized SQL,
unsafe-code denial, derived media keys, no server fetching of avatar URLs,
media sniffing/attachment isolation, dummy password verification, exact-origin
CSRF checks and post-hash password authority checks. The proposed removal of the
independent read-command guard and final media authorization check was rejected
for the reasons recorded earlier in this ledger.

Obsolete pre-v17 upgrade-success fixtures and pointer/full-rewrite helper assertions
are removed. Unsupported-schema rejection, rollback and current-baseline migration
coverage remain. The goal is complete in the working tree; this final stage has not
created an additional commit.
