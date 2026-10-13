# Testing

## Required checks

From the repository root:

```sh
rtk proxy ./auto/check.sh
```

This runs Rust formatting, Clippy with warnings as errors, Rust tests, TypeScript
checks, type-aware ESLint, web formatting, and web tests. Dependencies must be
installed first. To run individual checks:

```sh
rtk cargo fmt --all --check
rtk cargo test --locked
rtk cargo clippy --all-targets --locked -- -D warnings
rtk pnpm --dir web typecheck
rtk pnpm --dir web lint
rtk pnpm --dir web format:check
rtk pnpm --dir web test
rtk proxy ./auto/build.sh debug
rtk proxy ./auto/build.sh release
rtk pnpm --dir web test:integration
```

`auto/build.sh` installs dependencies, bundles the UI, and compiles Rust without
running source checks or tests in either mode. Release builds package `./chat`
and `./ui.tar.xz` containing `dist/`. Run `auto/check.sh` separately.
`auto/debug.sh` runs the existing debug binary and previews `web/dist-debug/`
at http://127.0.0.1:5173 with API/WebSocket proxying; it does not rebuild.
`auto/clean.sh` removes server/UI build artifacts and Vite caches, preserving
data, dependencies, and lockfiles.

The standalone `pnpm --dir web build` runs TypeScript, ESLint, and formatting
checks before Vite. Vite's transpilation is not a substitute for type checking.
Frozen pnpm and Cargo lockfiles make dependencies reproducible. The scripts contain
ordinary commands; RTK wraps their invocation only.

For formatting:

```sh
rtk cargo fmt --all
rtk pnpm --dir web format
```

## Coverage

Rust tests cover configuration validation, proxy trust, verified client-IP login throttling and spoofed/malformed forwarding headers, account permissions and deletion, room membership, private visibility, password/session behavior, retained history, reactions/replies/mentions, schema rejection, restart/move persistence, and failed-write rollback.

Web helper tests cover the shared request lifecycle (success, rejection, mismatched acknowledgements, send failure, disconnect and account promises), captured confirmations, private command routing, browser-local aliases, local command ordering, redaction, suggestions, Unicode mentions, notifications, browser timezones and daylight saving, fonts/storage failures, unread visibility, retention, and resynchronization races. Protocol/client tests exercise malformed nested JSON, frame variants, base-path URLs, credentials, abort signals, and API errors.

Build tests verify that both modes skip checks and that release packaging
contains the `dist/` directory with its files. Rust tests also verify that a poisoned engine
returns an unavailable response and never reuses potentially partial state.

`web/scripts/smoke.mjs` starts isolated debug/release processes on ephemeral ports and uses HTTP and real WebSockets. Every received frame passes the same runtime validator as the application. It checks authorization, commands, privacy, second-device directories, bounded room/private history, read cursors, account deletion, restart, API-only routing, base paths, trusted proxy/TLS headers, origin rejection, secure cookies, and logout disconnection. It never uses the working `data/` folder. Build both binaries first.

Current-schema persistence tests cover v8 restart, lazy history loading, state/cursor preservation, and rejection of every unsupported schema without modification. Command-family tests cover group filtering, removed syntax, and group-removal authorization.

The trust-model regressions additionally cover su group delegation and revocation,
protected su accounts (including disabled targets), creator ownership,
invitation-only rooms, per-chat command gates, read-only grants, cleanup age
constraints, cross-target authorization, private inspection, stable IDs,
ownership transfer, rename/cursor preservation,
owner delegation and revocation of invitations without enabling other management
permissions, loss of that delegation authority after ownership transfer,
room rename action/context/target gates, management bundles and ownership transfer,
Unicode names, no-op/collision validation, stable history/grants/read cursors,
name reuse, restart persistence and cursor/metadata/audit write rollback,
and compiled-policy rollback. Output regressions verify that @global and its
legacy alias resolve to the same scope, command requirements do not grant actions,
and assignment privacy and conditional cleanup limits survive formatting. The room
directory excludes other users' private pairs even with explicit inspection grants
or su membership. `/console` checks scoped grants, rejects extra arguments, and
does not change authorization or persisted state. Run
the manual optimized permission benchmark with:

```sh
rtk cargo test --release --locked permission_lookup_benchmark -- --ignored --nocapture
```

## UI verification

After changes to components/hooks, inspect both a wide desktop and compact view. Verify login, sidebar collapse and scrolling, room/private selection, local commands and hints, composing Unicode text, replies/reactions, settings, unread badges/jump, and reconnect behavior. Refresh must erase command output while restoring server chat history. Check browser errors and HTTP/WebSocket failures.

For production deployment, additionally verify the actual public hostname through Nginx/cloudflared: HTTPS redirects, WSS, session cookies, and the configured base path. Local forwarded-header checks do not exercise public DNS/TLS infrastructure. Validate Nginx configuration on the target host.

## Authorization coverage matrix

The trust suite checks every declared action against fresh user/admin/su/console
principals across server, owned room, foreign/private, account and unknown-resource
scopes. An explicit probe table covers denial of every registered command from
Command, room and private contexts for an account with no grants; adding a command
without a probe fails the test. HTTP history/read operations and snapshot payloads
are tested independently of command invocation.

Boundary tests cover direct versus group syntax, additive revoke behavior, scopes
named after groups, malformed requests, su-only delegation and protected accounts,
partial-authority invitations/group assignment, cross-target command borrowing,
message ownership and identity reuse, private visibility, read-only controls,
cleanup constraints, persistence failures, v8 restart and rejection of unsupported schemas.

A deterministic password-job test queues hashing behind a blocking worker, then
revokes the action or command, promotes the target to su (active or disabled),
replaces the target and regrants its privileges, revokes the session, disables
the actor, or revokes account-creation authority. Each case verifies the specific rejection and that
no stale operation modifies the resulting state. This test uses no timing sleeps.

These are behavioral and boundary checks, not a claim of 100% line/branch coverage
or a proof that every possible attack is excluded. Update this matrix and add
positive/negative cases whenever new permissions, commands or resource types are
introduced.

Rename completion tests cover room/user subcommands, existing account shortcuts,
free-form destination names, sudo wrapping and `x:room.rename` grants. The
integration harness verifies room rename broadcasts, stable IDs, history and
read-state preservation for two live clients, rejection of unauthorized renames,
and continued messaging in the renamed room.

Sudo regressions cover the two independent global grants, insufficient room
grants, temporary authority on success/rejection/storage failure, caller identity
in ownership/messages/audit, self-revocation, and denied nested commands. Password
jobs additionally recheck both sudo grants, actor session/disabled status, and
target identity after hashing. Frontend checks mask wrapped passwords and retain
wrapped command completion and routing.
Join regressions cover persisted sudo membership and participant grants, repeated
joins, access after sudo revocation/restart, and membership/authority rollback on
storage failure. Web command checks verify the wrapped result and snapshot updates.
The sudo command matrix covers every registered engine command with a caller
holding only sudo grants, including caller identity in reactions, replies,
private messages, ownership and audit entries. Transport tests cover configuration,
password/account commands, clear/logout, and remote bot configuration; bot results
are withheld if either sudo grant is revoked while awaiting the runtime.
Sudo debug checks cover the successful tab-local toggle and invalidation when
either global sudo grant is revoked.


## Persistence benchmark

Run `rtk proxy python3 auto/benchmark-storage.py` to build the baseline commit and
current code against identical isolated fixtures. It prints send/reaction p50/p95,
separate snapshot/serialization timings, WAL bytes, changed-row counts, and isolated
process peak RSS. It uses temporary databases, never `data/`. See
[Storage benchmarks](storage-benchmarks.md) for methodology and recorded results.
Targeted-write tests reject unrelated payload/policy updates and inject failures at
message insertion/eviction, counter/revision changes, reaction insertion, and new
private-conversation creation; disk state, cache, sequences and revisions must all
roll back. Startup validation tests verify malformed current-schema policy and cursors fail without rewriting stored metadata.

## Bounded history regressions

`engine/history.rs` covers direct deep-range reads, ascending/descending traversal
with sequence gaps, invalid bounds, empty-edge direction flags, large payload
progress, around-anchor fallback, revision/cursor invalidation, retention changes,
lazy startup, indexed deep-range query plans, streaming rename batches and name
reuse, cache entry/byte/page budgets, current-schema restart, four
simultaneous readers, and WAL reader/writer overlap. Existing regression suites
also verify private history survives account renames and persistence failures.
Web server tests check overlapping engine readers and bounded work admission.

`web/tests/historyCache.test.js` covers scrolling beyond 1,000 messages and back,
overlap/reaction replacement, directional byte eviction, LRU entry/view/byte
budgets, bounded snapshot tails, incremental update isolation, stable identity
rejection and private pair routing. `web/tests/virtualTimeline.test.js` exercises
viewport/overscan bounds, exact variable-height boundaries, spacer totals and
anchor preservation after measurement, prepend and opposite-edge eviction. These
geometry tests do not replace browser DOM/ResizeObserver verification on devices.
`web/tests/timelineMotion.test.js` distinguishes live arrivals from backfill,
detects remote edits and retractions, and skips text animations for reactions
and local output. Browser motion checks should cover arrivals, last/interior
deletions (including the only remaining message), long/short remote edits, rapid updates, inline editor transitions,
history paging/view switches, compact widths and reduced motion. Confirm that
the bottom remains anchored, readers of older history are not pulled down,
exit copies have no duplicate IDs/read targets, and animations/copies are
removed after the transition.

The repeatable browser fixture at `web/scripts/motion/index.html` uses the actual
message components and hooks with synthetic data; it needs no backend and never
writes chat history. Start `rtk pnpm --dir web dev`, open
`http://127.0.0.1:5173/scripts/motion/index.html`, and select **Run all cases**.
Repeat with **Compact** enabled in Chromium and WebKit. Its 20 cases cover
group/date-header changes, deleting the only message, overflow becoming a short
history, 1,000-row histories, rapid deletions/arrivals, an open inline editor, edits, paging, view switches
and reduced motion. Assertions inspect text positions on the first changed frame,
direction/overshoot, paint clipping, scroll anchoring and animation cleanup;
checking only outer row positions or final state misses visible text jumps.

Also verify the full HTTP/WebSocket path with isolated `CHAT_DATA`: send three
consecutive messages, then delete the first. Delay history responses during this
check so the pending refresh is visible. The viewport must remain stationary
through the refresh, and the next message's text must start from its old screen
position when it gains an author/date header. Initial loading and explicit
paging must still display their progress indicator.

The HTTP/WebSocket smoke suite additionally exercises sequence ranges, before,
after, around, concurrent page requests, malformed/unsafe bounds, authorization,
logout and cache hits using isolated temporary databases. Run the required checks
and integration suite after rebuilding both binaries. No line/branch coverage
percentage or production load capacity is claimed by these behavioral tests.

Command syntax regressions cover `/history`, current-conversation ranges and
`/history #room`, default latest-100 output, rejection of removed aliases, target
privacy and sudo inspection without joining or retaining elevated authority.

## Bot coverage

Rust bot tests cover explicit scope resolution; safe direct defaults and filtered command lists; admin-only management; protected active/disabled su targets; ordinary grants/revokes; room/history privacy; mentions/replies/reactions/read cursors; literal slash text; credential hashing, labels, rotation/revocation; disable/enable/rename/delete and name reuse; creation/credential/message rollback; v8 restart; HTTP bearer-only boundaries, malformed requests, payload limits, Unicode, account-wide input budgets, shutdown and base paths.

The integration harness additionally connects a real bot WebSocket in production under /commonroom/, validates every frame with the frontend guards, checks normal mention delivery and bot replies, live permission revocation, credential revocation/disconnection, and history recovery after restart. It uses isolated temporary data.

Trusted direct bot transport regressions additionally check production HTTP/WS without a forwarded-protocol header, valid/invalid/missing credentials, IPv4-mapped loopback, missing/untrusted socket identity, spoofed forwarding headers, forwarded-http rejection, base paths, unchanged browser HTTPS requirements, and direct bot message sends. The real socket integration suite connects the bot using direct WS and verifies revocation/disconnection.

Remote bot configuration regressions cover separate read/write account grants, command/context gates, cross-account denial, lifecycle restrictions, protected/disabled targets, result authorization rechecks, offline/disconnected runtimes, duplicate handler registration, malformed/oversized responses and timeout/cancellation. The real production WebSocket harness checks registration, set/fetch round trips, runtime rejection, delegated access/revocation and unsolicited responses.

Editing regressions cover ownership, separate action/command grants, room and
private scope isolation, Unicode limits, expired messages, renamed authors,
identity/format/reaction/quote preservation, mention recalculation, history
cache invalidation, restart persistence, transactional failure rollback, and
unchanged edits without storage writes or revision changes.
Web tests cover edit visibility, malformed edited markers, and request failures;
the integration harness checks edit broadcasts to a second client and history.

Edit-any regressions cover scoped action/read/command gates, delegation and
revocation, ordinary admin defaults, cross-room/private isolation, disabled
editors, su/console/sudo authority, author/participant preservation, and
transactional rollback. UI checks cover sizing to all text on entry, growth and
shrinkage while typing, and reflow at compact widths.

Editor scroll geometry tests cover keeping the last message's footer anchored
when the textarea grows/shrinks, temporary measurement collapse, and scroll
bounds. Browser checks exercise this with the virtualized conversation list.
