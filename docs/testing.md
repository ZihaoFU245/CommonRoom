# Testing

## Required checks

From the repository root:

```sh
rtk proxy ./auto/check.sh
```

This runs Rust formatting, Clippy with warnings as errors, Rust tests, integration-harness syntax, TypeScript
checks, type-aware ESLint, web formatting, and web tests. When the optional
`simpleBot/` demo is present, its type checks and unit tests run too. Dependencies must be
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

Current-schema persistence tests cover v10 restart and v9 migration, lazy history loading, state/cursor preservation, and rejection of every unsupported schema without modification. Command-family tests cover group filtering, removed syntax, and group-removal authorization.

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
cleanup constraints, persistence failures, v10 restart and v9 migration and rejection of unsupported schemas.

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
password/account commands and clear/logout. Removed remote bot configuration
commands are rejected, including sudo-wrapped requests.
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
Web server tests check overlapping engine readers and bounded work admission,
concurrent baseline sharing, credential expiry on cache hits, and invalidation
on message, permission and session changes. History tests verify shared page
slices and exact wire budgets, plus unread-cache cursor/revision replacement
and bounded recency. Presence tests check first/last socket transitions.

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

Lifecycle tests cover scoped safe defaults, admin/su boundaries, credentials,
revocation/disable/delete, name reuse, persistence and failed-write rollback.
Frozen v1 tests cover stable targets through rename/recreate, atomic retry
receipts, expiry/capacity, replies, desired-state reactions, read cursor retries,
permissions, Unicode, body limits, typed failures and production ingress.
Discovery tests check credential/access rechecks, socket lifetimes and bounded
catalogues. Removed configuration RPC frames are rejected.

The integration harness uses real production HTTP/WebSockets under a base path.
Every bot response and frame passes the published JSON Schema. It checks activity
control, command rejection, permission/credential removal, stale destination IDs,
reference-worker catch-up and keyed sends across server restart. Isolated
temporary data keeps working credentials/history untouched.

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


## Bot contract guardrails

`web/tests/botContract.test.js` validates the v1 schema and
payloads in both socket directions. `web/tests/botClient.test.js` covers bounded
retries, multipage recovery, duplicate signals, original outbox payload/key reuse,
account mismatch and expired/corrupt checkpoints. These run with the normal web
suite. V1 is the sole bot API namespace. Its refactored credential, revision and shared
unread rules are tested with the worker; update the schema and consumers together. Rebuild both binaries before running the real
integration suite.

## Optional simpleBot checks

When the separately supplied `simpleBot/` directory is present,
`auto/check.sh` also checks `simpleBot/tsconfig.json` and runs the worker's unit
and transport tests. They use temporary Deno KV databases and local loopback HTTP
servers, with no model credentials. Regression coverage includes multipart
restart recovery, repeated edits, in-flight retraction, multipage catch-up,
receipt expiry, malformed responses, safe retry boundaries, streamed UTF-8 and
error events, and activity capability/credential checks.

After `rtk cargo build --locked`, run the real entrypoint test:

```sh
rtk proxy env BOT_INTEGRATION=1 deno test --unstable-kv -A --no-check simpleBot/test/integration.test.ts
```

It starts an isolated Rust server under `/commonroom/`, a worker process, browser
and bot sockets, and a keyless model stub. It verifies authentication, activity,
Markdown defaults and opt-out, live model configuration, edit replies, reactions,
and restart without duplicate output. Working `data/` and `simpleBot/data/` are
never used by tests.

Command discovery regressions cover typed v1 frames, catalogue bounds, duplicate
publication ownership, unsolicited/stale responses, request expiry/capacity,
scoped credentials without configuration authority, member-list privacy,
revocation, and browser session authorization. Browser tests check mention-prefix
completion, Unicode bot names, argument preservation and malformed metadata.
Subcommand tests cover three-level descent, full usage paths, sibling uniqueness,
depth/total-node limits, malformed children and caret suffix preservation.
The simpleBot integration test verifies registration/query/response, browser
invalidations, configuration-driven catalogue refresh, and disconnect/reconnect
cleanup against a real isolated server without model calls for discovery.
It also executes native model tools as the bot account, checks owner/admin controls independently, and verifies that private configuration never enters model context.
Bot-invocation regressions preserve the full command and arguments in literal
transcript rendering, and the real integration test checks that a second room
member receives the full invocation and retained history keeps it intact.

Native tool regressions cover buffered and streamed function calls, fragmented
arguments, tool results, non-retryable command POSTs, saved mutation results,
duplicate suppression and uncertain-effect restart/pruning behavior. Server tests
cover current command/target grants, scoped-token isolation, bot admin access,
verified sender roles, policy-inspection privacy, owner transfer/rename/deletion,
and private-catalogue filtering against current ownership.

SimpleBot configuration regressions cover quoted/empty values, Unicode and
newline JSON round trips, CLI JSON output, invalid-file preservation, stale
writers and lock contention, atomic reload and environment seed replacement.
The real worker integration verifies prompt reload and transport reconnection
after acknowledgement. `web/tests/wsProxy.test.js` exercises browser/server TCP
half-close cleanup through the actual Vite proxy.

Command receipt regressions verify concurrent replay, payload conflicts,
authenticated outcome lookup after target deletion, terminal rejection,
revocation, restart, receipt-write failure rollback and transactional v9 upgrade.
History tests load pages around message IDs and reject mixed cursor modes.
SimpleBot regressions cover public help/ping without model calls, fresh context
on restart, quoted-message retrieval, model-driven paging, bounded/deduplicated
context, keyed pending-command recovery, scoped credential issuance/revocation,
typed room/DM creation and desired-state reactions.

Agent-turn regressions verify sender usernames in current model context, one
reply despite overlapping signals/reconnect, outbox-only reply publication,
silent decisions (with and without tool effects) across restart, and edit races
between queuing and locating input. Both buffered and streamed models accept
explicit empty completions without treating transport truncation as silence.

Autonomous worker tests cover fixed three-second windows under concurrent
speakers, minimum turn spacing, combined plain output, independent targeted
replies, silence, own-output suppression, runtime enable/disable, foreign private
chat isolation, source retraction, and interrupted KV-backed response plans. Failure tests cover stalled/erroring providers, ignored
cancellation, late tool rejection and failed command-receipt lookups while new
input continues. Buffered and streamed models exercise 16 tool calls and terminal
response-plan tools. The real worker integration checks multi-user autonomous
bursts, reply targets, silence and restart against an isolated server/model stub.

SimpleBot requires Deno 2.5+ for worker tests. Durable-storage regressions cover
restart recall, silent group memory, conversation/account isolation, conditional
updates, pagination, nested targeted writes, private database permissions,
interrupted large-plan staging, and stale-writer rejection. Source checks use
the repository's TypeScript tools; Deno executes the tests with `--unstable-kv`.

SimpleBot execution tests run real restricted Deno subprocesses in temporary
workspaces. They cover JS/TS and top-level await, persisted scripts and relative
imports, folder-bound reads/writes, outside-file/import denial, environment and
nested-process denial, FFI/system permission denial, symlink handling, output
bounds, cancellation, timeouts and the actual worker permission profile. Local
HTTP fixtures check fetching and module imports. `BOT_EXEC_HTTPS=1` enables the
optional public HTTPS check in `simpleBot/test/execution.test.ts`. The real worker
integration saves and reruns a script across restart through native model tools.
Prompt tests cover autonomous on/off, both DM trigger settings, natural group
participation, storage/network guidance and internal-information instructions;
they validate prompt construction, not a provider's compliance.

Background-tool regressions exercise buffered and streamed inference while a
tool is pending, explicit result waits, automatic success/failure delivery,
premature final drafts, finishing-plan barriers, tool budgets and cancellation.
Resource tests check overlapping readers, writer exclusion, FIFO fairness,
cancelled queued writes, lock release after failure and shared workspace locks
across completions. A real restricted Deno subprocess holds a local HTTP request
open while the model reads native KV, then finishes or is cancelled; tests verify
the result, filesystem effects and runtime-cache cleanup. Context compaction
preserves background job IDs/status and valid JSON while bounding result text.
The real worker integration's model fixture understands job handles and waits
before responding, including saved-script execution across restart.
