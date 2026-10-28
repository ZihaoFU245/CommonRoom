# Testing

## PostgreSQL backend and SQLite transfer

The ordinary Rust suite runs SQLite without an external database. PostgreSQL
contracts are opt-in ignored tests that create/drop unique schemas in an explicitly
selected test database. Use a disposable PostgreSQL 18 container, not a production
database. Run them serially because database ownership is deliberately exclusive:

```sh
container run -d --name chat-postgres-test --memory 1G --cpus 2 \
  -e POSTGRES_USER=dev -e POSTGRES_PASSWORD=devpass -e POSTGRES_DB=chat_test \
  -p 127.0.0.1:55432:5432 docker.io/library/postgres:18
export CHAT_TEST_POSTGRES_URL='postgresql://dev:devpass@127.0.0.1:55432/chat_test'
rtk cargo test --locked postgres_ -- --ignored --test-threads=1
rtk cargo build --locked
rtk proxy python3 auto/test-postgres.py
container stop chat-postgres-test
container delete chat-postgres-test
```

Wait for PostgreSQL readiness before running tests. These contracts exercise
SQLite transfer of all 14 tables, unchanged source bytes, target rollback on a
checksum mismatch, rejection of active/nonempty/unsupported targets, preserved
password hashes/sessions/retry receipts/cursors, Unicode and embedded NUL text,
media lifecycle and download revalidation, native command/message failure rollback,
repeatable reader snapshots, restart and connection-loss recovery. Container
cleanup is required even when a test fails. The existing SQLite suite retains its
integrity, crash, rollback, query-plan and mutation-cost checks.
Native-dispatcher tests also hold a PostgreSQL conversation row lock and require
an independent conversation's message transaction to complete, race cursor
advances, distinguish idle pool replacement from ownership loss, and check SQLite
single-writer capacity and cancelled acquisition recovery. Stale message plans
must fail without overwriting a committed edit.

`auto/test-postgres.py` requires the target's default schema to be empty. It
provisions a temporary SQLite server through the real stdin console, verifies HTTP
login, runs the binary's transfer command, then checks PostgreSQL HTTP sessions,
password login, restart and graceful shutdown twice. It leaves the migrated test
tables in PostgreSQL for container removal; use a fresh container to repeat it.
It also verifies that a rejected second process cannot rewrite saved configuration
for either backend, a URL alone cannot select PostgreSQL, and missing/empty URLs
cannot silently fall back to SQLite after PostgreSQL is selected in configuration.
Configuration regressions cover legacy SQLite defaults, backend changes during
startup, and rejection of database URLs/passwords and S3 credentials in saved JSON.

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

Dependency advisory CI runs `python3 auto/audit.py` with Python 3.11+, Git,
Cargo and cargo-audit 0.22.2. The script fetches a fresh RustSec database and audits
the locked workspace graph with all workspace features and target platforms.
Use `--db PATH` for an already-fetched advisory database. It creates a temporary
lockfile projection without modifying `Cargo.lock` or ignoring advisory IDs.
Cargo's lockfile also records inactive optional SQLx MySQL/RSA dependencies:
the unused RSA entry has RUSTSEC-2023-0071, with no fixed version. These entries
are excluded only while absent from the resolved graph; enabling them makes the
audit include them automatically. Do not claim the entire raw lockfile is clean.

Review regressions cover an HTTP future aborted inside COMMIT, one forced owner
probe per append, configurable PostgreSQL pool limits, batched media acknowledgement
rollback on both backends, and cleanup recount when an empty target receives a
message during the count. Publication recovery tests force a committed mismatch,
verify authoritative reload and a subsequent write, and verify invalid persisted
metadata remains unavailable. A shutdown test queues recovery behind admission
and requires it to finish before checkpointing. Real ordered-worker panic tests
require subsequent reads and writes to fail closed.

The ignored `snapshot_fanout_shares_cold_reads_and_encoded_content` fixture checks
100 viewers across 30 rooms, shared page identity and one encoding per page.
See [Storage benchmarks](storage-benchmarks.md#shared-snapshot-fan-out).
`auto/debug.sh` runs the existing debug binary and previews `web/dist-debug/`
at http://127.0.0.1:5173 with API/WebSocket proxying; it does not rebuild. It defaults
to SQLite/no S3, with opt-in `--with-s3` and `--with-postgres` flags. See
[Development](development.md) for the generated per-run config and data folder.

`auto/clean.sh` removes server/UI build artifacts, Vite caches, and
`tmp/rustfs-local/` (including local buckets, logs and temporary files).
Stop debug and RustFS first. It preserves `tmp/rustfs`, other `tmp/` files,
chat data, dependencies, and lockfiles.

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

`virtualTimelineHook.test.js` checks render suppression within a virtual window,
latest-position paging anchors, observer-based row resizing and bottom following.
Its DOM-commit simulation clamps scroll offsets before layout effects to cover
history replacement jumping forward or backward to a different retained message.
History navigation tests check the three-viewport shortcut and one-use unread targets,
animated virtual scrolling, measurement updates, interruption and reduced motion.
Regressions cover keyboard paging on short history, editor-key isolation and
deleted/expired unread destinations both within loaded history and after paging.
Browser regressions cover unread entry, restored positions between rooms,
automatic paging without manual controls, live unread notices and jumps to unread
or the latest messages in wide and compact layouts.
Notice-motion regressions sample viewport height and scroll position each frame
during entry and exit, including concurrent smooth scrolling, and verify that
unread updates reuse the existing notice node.
Floating-button checks also require unchanged message space, hit-testing through
the transparent surrounding area, one clickable button and no unread count.
Open `/scripts/regressions/index.html?test=unread%20notice%20motion` in Safari
to run just the notice checks, including shared 120 ms timing, slide-only keyframes,
monotonic movement and disabled elastic overscroll.
`observedBorderBox.test.js` checks border/padding accounting and geometry fallbacks.
`connectionHook.test.js` covers resume-event coalescing, hidden-page cancellation,
socket-baseline recovery and revoked-session handling. Media-cache tests cover
reuse of cached response sizes, byte limits after restart, overlapping worker
lifetimes, recency and storage-error recovery. `timelineScroll.test.js` covers
temporary animation overflow retaining bottom intent and manual release.

Browser command-output regressions submit `/debug on` without a command grant in
rooms, Command view and private chats, then submit another message or command.
The room case uses 1,000 messages and exercises the first lazy formatter load:
command output must stay mounted while loading so virtual row measurements cannot
oscillate between Suspense fallback and content heights and hang the tab.

Blank-message regressions share Unicode cases between the browser and server.
They reject whitespace, control, invisible-format and placeholder-only text on
room/private sends, replies, edits and bot sends, preserve real Unicode text,
and verify that previously stored blank messages remain readable and replyable.

Rust tests cover configuration validation, proxy trust, verified client-IP login throttling and spoofed/malformed forwarding headers, account permissions and deletion, room membership, private visibility, password/session behavior, retained history, reactions/replies/mentions, schema rejection, restart/move persistence, and failed-write rollback.

Web helper tests cover the shared request lifecycle (success, rejection, mismatched acknowledgements, send failure, disconnect and account promises), captured confirmations, private command routing, browser-local aliases, local command ordering, redaction, suggestions, Unicode mentions, notifications, browser timezones and daylight saving, fonts/storage failures, unread visibility, retention, and resynchronization races. Composer tests cover clipboard images (first image wins, non-image pastes fall through, unnamed screenshots get a typed filename) and the attach availability rule shared by the picker and pasted images. Markdown tests cover browser-local HTML opt-in persistence, mixed HTML/Markdown, literal code, malformed markup and blocking active content, arbitrary attributes, unsafe links and images. Protocol/client tests exercise malformed nested JSON, frame variants, base-path URLs, credentials, abort signals, and API errors.

Build tests verify that both modes skip checks and that release packaging
contains the `dist/` directory with its files. Rust tests also verify that a poisoned engine
returns an unavailable response and never reuses potentially partial state.

`web/scripts/smoke.mjs` starts isolated debug/release processes on ephemeral ports and uses HTTP and real WebSockets. Every received frame passes the same runtime validator as the application. It checks authorization, commands, privacy, second-device directories, bounded room/private history, read cursors, account deletion, restart, API-only routing, base paths, trusted proxy/TLS headers, origin rejection, secure cookies, and logout disconnection. It never uses the working `data/` folder. Build both binaries first.

Current-schema persistence tests cover HEAD SQLite v19 initialization and restart, lazy history loading, state/cursor preservation, and unchanged v19 reopen, independent PostgreSQL schema initialization, and rejection of every unsupported schema without modification. Object-command tests cover scoped listings, removed media-grant arguments, and group-removal authorization.

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
and assignment privacy and cleanup limits remain separate from permission formatting. The room
directory excludes other users' private pairs even with explicit inspection grants
or su membership. `/console` checks scoped grants, rejects extra arguments, and
does not change authorization or persisted state. Run
the manual optimized permission benchmark with:

```sh
rtk cargo test --release --locked permission_lookup_benchmark -- --ignored --nocapture
```

## UI verification

Save-feedback browser regressions exercise actual color HTTP requests returning
403/502/503 and network errors, retaining the selected color and showing an alert
beside Apply until a successful retry. The picker remains open while saving.
Account-command rejection/disconnection retains edits and exposes an alert;
failed avatar uploads retain the selected image and crop for another attempt.
Save feedback keeps actionable validation/permission errors, explains expired
sessions and connection/timeouts, and replaces server/storage diagnostics with
plain retry instructions. Upload errors do not ask users to configure CORS.

The composer IME browser regression replays Safari's unmarked candidate keys,
key code 229, and confirming Enter immediately after compositionend. It also
checks consecutive compositions, blur recovery, Shift+Enter and a deliberate
later Enter; candidate confirmation must not submit or consume hint shortcuts.

Reply-quote browser regressions cover originals outside the virtual window and
outside loaded history. A known original sequence requests one around-page;
legacy quotes without a sequence can still reveal a loaded virtual row.

After changes to components/hooks, inspect both a wide desktop and compact view. Verify login, sidebar collapse and scrolling, room/private selection, local commands and hints, composing Unicode text, replies/reactions, settings, unread badges/jump, and reconnect behavior. Refresh must erase command output while restoring server chat history. Check browser errors and HTTP/WebSocket failures.

The isolated browser fixture at `/scripts/markdown/` on the Vite dev server
mounts 1,000 messages with counter, snake, safe HTML and isolation-check payloads.
Allow HTML and LaTeX start off. It does not access account data.

- Check history replacement stability must retain message 500 after narrowing the
  cached history to messages 450–550, with at most 1px offset drift. Check room
  update stability exercises rolling arrivals and repeated last-message resizing
  while following the bottom. Repeat both checks in desktop and compact layouts.

- With all options off, HTML is text; optional HTML/TeX/KaTeX/frame modules do not
  load. Plain-text chats also leave Markdown-it unloaded. Markdown images remain
  validated, lazy, async and no-referrer, with rejected URLs falling back to alt.
- Allow HTML offers the same isolated Render HTML preview for passive markup and
  active HTML/CSS/JavaScript. No HTML renders automatically; no iframe mounts until
  the user selects Render HTML. Show text returns to the scrollable source panel.
- Load long code and long plain text: each should start with only the top 256px
  visible and a Show more button. Expand and fold with mouse and keyboard; verify
  the entire body appears, reduced motion skips animation and folding keeps the
  footer and following messages in place while text compacts from above. The
  control must match the small borderless edited label with a down/up arrow.
  Use Load very long code as well: expansion must spend multiple frames revealing
  the visible viewport, even when the full body exceeds 3,000px.
  Check fold motion samples intermediate expansion/collapse heights and reports
  following-message drift; expect multiple animation frames and at most 1px drift
  in desktop and compact layouts when the scroll range permits anchoring. Check
  wide/compact layouts, delayed Markdown/image sizing,
  virtual scroll away/back, editing, conversation switches and window eviction.
  Short messages must have no dropdown and editors must remain fully visible.
- Use Check delayed image motion at the start and end in wide and compact layouts. It loads a local generated image after the initial layout and samples painted heights and scroll placement: expect multiple intermediate frames, at most 1px bottom gap when following the newest message, and at most 1px anchor drift while reading older messages. Repeat while HTML/frame previews and math finish loading. Manual wheel/touch/keyboard scrolling must release bottom following, and reduced motion must apply the final height immediately. Resize the viewport during a load and expand/fold before it finishes; there must be no nested resize animation or stuck fixed height. `contentResizeMotion.test.js` covers mount suppression, delayed growth/shrinkage, retargeting, stale completions, reduced motion, reflow, explicit folding, offscreen updates and bottom ownership.
- In mixed messages, verify surrounding Markdown, inline emphasis, lists and math
  remain formatted while only HTML enters the viewer. Multiple separated portions
  need independent consent; adjacent HTML/CSS/scripts share a working preview.
  Editing any part resets all choices. Code fences/spans remain literal.
- HTML starts as a compact cover with Show text / Render HTML controls. Show text
  toggles a source panel capped at 400px with horizontal and vertical scrolling.
  With Allow HTML off, source uses the same bounded panel directly. Check smooth
  panel expansion and folding, keyboard controls, reduced motion and compact widths.
  Use Counter at end to verify preview expansion and late frame resizing preserve
  the scroll anchor; manual scrolling must release it. Verify the counter
  increments/decrements/resets and snake responds to keys only inside the frame.
  A full indented HTML document must render as a document, not a Markdown fence.
- Code fences/spans and autolinks remain literal. Unknown benign wrappers retain
  children; forged private image/math markers never resolve. Failures retain text.
- Frame isolation checks must show parent DOM, localStorage, cookies and fetch
  blocked, with `self.origin` equal to `null`. Check the sandbox has allow-scripts
  only. Spoofed height messages from other windows must be ignored; all reported
  heights are bounded. Forms, popups, external resources and top navigation stay
  blocked. Run the same checks without HTTP CSP headers to exercise the meta CSP.
- Check keyboard focus/aria-pressed, wide/compact views, edits, Security toggles,
  conversation switches, window eviction and refresh. Each invalidates consent.
  Virtual scrolling preserves the choice but restarts a remounted frame; cached
  shell transfers must be zero on remount, with no polling or script/CSS requests.
- Verify image, math and frame resizing keeps timeline rows aligned and anchors
  stable. Check the built UI under production CSP and a non-root base path. Ensure
  the exact frame location has its own policy/cache headers and the main app keeps
  strict script/style headers. Production bundle tests check the initial import
  graph excludes Markdown-it, HTML, TeX, KaTeX and frame engines and packages the
  single standalone frame resource.

Math regressions cover dollar/bracket delimiters, inline/display formulas,
code isolation, malformed expressions, expansion limits, blocked HTML/network
extensions, independent preference persistence and both plain Markdown and safe HTML rendering.
In the browser, verify the Markdown checkbox and `/markdown` stay synchronized,
LaTeX toggles existing Markdown immediately, refresh preserves both preferences,
and bundled math CSS/fonts work with Allow HTML disabled.

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
cleanup constraints, persistence failures, HEAD SQLite v19 initialization and restart and rejection of unsupported schemas.

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

Run `rtk proxy python3 auto/benchmark-storage.py` to compare committed HEAD and
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
verified sender roles, policy-inspection privacy, owner transfer/user rename/deletion,
and private-catalogue filtering against current ownership.

SimpleBot configuration regressions cover quoted/empty values, Unicode and
newline JSON round trips, CLI JSON output, invalid-file preservation, stale
writers and lock contention, atomic reload and environment seed replacement.
The real worker integration verifies prompt reload and transport reconnection
after acknowledgement. `web/tests/wsProxy.test.js` exercises browser/server TCP
half-close cleanup through the actual Vite proxy.

Command receipt regressions verify concurrent replay, payload conflicts,
authenticated outcome lookup after target deletion, terminal rejection,
revocation, restart, receipt-write failure rollback and current-schema restart.
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
Model error regressions preserve HTTP 400/422 through streamed error-body
inspection, including single-call ticks, without cancelling a consumed stream,
retrying ordinary denials or logging private provider response bodies. Optional
stream-usage rejection still exercises the compatibility fallback.
Tick diagnostic regressions distinguish multiple/unexpected calls, invalid JSON,
invalid decisions and accepted final/structured responses in streaming and
buffered modes. They verify conversation/tick correlation, exclusion of diagnostic
IDs from the provider payload, and absence of prompts, reply/argument content,
reasoning, credentials and arbitrary unknown names/fields from logs. Loop tests
also check that validation repair logs its scheduled retry and reason.
Tick request regressions omit forced tool selection for thinking-model
compatibility while preserving the configured reasoning mode. Ordinary final
text/silence ends a single tick. Native-only and mixed native/finish_tick responses
become durable job plans; duplicate representations execute once, and malformed
plans start no effects. Streamed and buffered regressions cover Unicode action
IDs, precise validation-repair feedback without payload leakage, native reference
mapping and distinct native intents. Loop regressions check consumed job watches
across restart, job/wake diagnostics, preservation of all admitted inputs on job
continuations and inclusion of the configured prompt on each request.
Failure-recovery regressions reproduce repeated unknown `execute_code` arguments
in buffered and streamed responses, check schema-specific repair feedback,
verify one final tool-free recovery tick, and then process a fresh message.
Queued input cannot take over an ongoing repair or inherit exhausted retries,
including in autonomous groups. Failed recovery starts no effects and leaves
later input eligible. Omitted empty tick lists are accepted while invalid
explicit lists remain rejected. Multi-input plain/mixed group plans merge into
one output and retain reply ownership across restart; delivery diagnostics
contain IDs rather than message bodies.

Autonomous worker tests cover configurable fixed windows (including the former
three-second setting), the new one-second default, zero delay and mention bypass under concurrent
speakers, minimum turn spacing, combined plain output, independent targeted
replies, silence, own-output suppression, runtime enable/disable, foreign private
chat isolation, source retraction, and interrupted KV-backed response plans. Failure tests cover stalled/erroring providers, ignored
cancellation, late tool rejection and failed command-receipt lookups while new
input continues. Buffered and streamed models exercise 16 tool calls and terminal
response-plan tools. The model's server-command tool mirrors the fetched catalogue:
granted commands, including `/reply` and `/tell`, are offered and executed, a
command outside the catalogue is refused before any effect, and a credential that
cannot read the catalogue loses the tool with a logged warning. The real worker
integration checks multi-user autonomous bursts, reply targets, silence and restart
against an isolated server/model stub.

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

SimpleBot event-loop regressions in `simpleBot/test/loop.test.ts` cover one
provider call per tick, explicit continuations, one-shot deadlines across restart,
fast job-completion races, silent background work, cross-conversation inference,
responsive local controls, serialized inference within a conversation, stale
watches/timers, retraction cancellation, bounded self ticks, atomic rejection of
invalid plans, distinct effect keys across ticks, bounded result paging, admission backpressure,
token budgets, checkpoint/deadline races and lossless KV v1→v2 migration.
Terminal failures spend one bounded tool-less recovery tick, and a second failure
gives up without another call; a tool-budget stop publishes the decision's own
messages instead of failing the input, and spends that recovery tick when nothing
could be published. Streamed tool deltas tolerate a gateway repeating ids/names
and omitting the index after the first chunk. Only `finish_tick` is advertised:
tests assert the request body's tool list and that the nested action schema still
names every tool and preserves its description. Mixed-call regressions normalize
both representations before deduplication while retaining distinct native intents.
Unknown argument fields are dropped with a host-owned label and
a missing or shorthand continuation defaults to `wait`, both covered without
leaking model field names or values; fenced, prose-wrapped and double-encoded
decisions are repaired, while unparseable arguments still fail. Rejected requests
log status, body size and allowlisted `error.type`/`code`/`param` labels and never
the provider body. Privacy regressions reject arbitrary values and log newlines
in each field in buffered and streamed modes; parameter indices become `[]`.
An HTTP failure still cancels a body that never ends.
Cache regressions vary reply IDs and message limits without changing advertised
tools or losing measured token baselines. Loop tests verify live grants refresh
without rewriting the instruction prefix, overlapping catalogue/history reads,
suffix placement of repair feedback, and atomic rejection of jobs when the host
reply limit is exceeded. Telemetry tests distinguish preparation/inference time
and require first-delta measurements only for streamed responses.
Input-validation tests cover dense range sharing, four-read sparse concurrency,
abort before later groups, deleted/edited/mismatched IDs, byte-limited paging,
revision conflict retries and refusal to publish mixed revisions. Reply latency
tests cover shared group output, final multipart acknowledgement, retry call
counts, edits, unknown timings after restart/eviction, formatted Markdown tables
and bounded percentile samples. Loop tests verify one dense read for a multi-input
batch and two model steps for three independent jobs followed by one answer.
History reuse is covered separately: an unchanged content revision returns the
verified prefix without re-reading retained history, an append leaves the first
chunk byte-identical, and a revision change revalidates.
Reply-ownership regressions reject different second answers to one input,
including job completion, delayed continuation and restart; multiple messages
for one input within one tick are combined. Distinct new inputs
and unanswered group inputs remain eligible. Old v1 group-decision fixtures
cover saved silence and a lost send acknowledgement, verifying recovery without
inference and reuse of the original outbox receipt key. Recovery also releases
unfinished old batch members and avoids staging v2 output again when obsolete
v1 batch metadata remains.
The actual worker integration gateway emits finish_tick decisions for buffered
and streaming requests and verifies native tools, output and restart recovery.

Shell regressions in `simpleBot/test/shell.test.ts` exercise startup-only
capability parsing, task-launch permission selection, default denial,
configuration rejection, binaries/pipes/redirects, inherited HOME, absolute and
parent working directories, quick-result grace, output bounds, timeout and
process-group cleanup. Loop tests cover responsive controls while a shell job
outlives inference settings, wait-expiry checkpoints and restart without shell
access or command replay. The real worker integration includes a native shell
command launched with `--with-shell`. These tests exercise process supervision;
they do not install or validate a third-party sandbox.

Correctness/availability regressions cover the actual HTTP 401 message and non-JSON
proxy failures, per-account session eviction after 256 login attempts, disabled
administrators in last-admin guards, indexed rename isolation and indexed references. Hook tests drive continuous revision changes during explicit
paging and continuous snapshots during read acknowledgements; retries stay bounded,
the original range is preserved, and live tails remain in the ordering ledger.

Performance regressions guard unrelated entity/state writes during retraction and
cleanup, audit retention without rewriting retained entries, audit write rollback, startup expiry pruning, configuration ceilings, shared
snapshot buffers after read-only commands, credential-isolated bot cache reuse,
revocation checks, prepared-send races and fail-closed rollback reload failures. Snapshot synchronization tests reject
older socket baselines after newer HTTP data and accept a restarted instance.
The motion fixture passes all 20 scenarios in wide and compact modes, including
1,000-message windows, rapid changes, edits, paging and reduced motion.

Latency regressions cover delayed catalogue replacement, bounded/prefetched
browser metadata, stale-result rejection, transient failures versus authoritative
access denial, overlapping message acknowledgements and exclusive commands.
SimpleBot tests assert socket-baseline reuse, append-only request counts, mixed
edit fallback, policy-bound catalogue reuse and provisional output before durable
publication. The actual worker integration verifies draft activity reaches browser
sessions without entering history and clears on idle.

UI responsiveness regressions hold history responses pending while switching
between cached conversations, reject inaccessible/replaced identities, and verify
that complete snapshot tails skip HTTP. Additional checks cover retention/tail
preview reconciliation, stable sequence/pixel scroll restoration, adaptive paging
bounds, parallel reconnect validation, current revocation and stale-check races.

Media-specific engine/web regressions run in the normal suites. For real RustFS
HTTP/WebSocket upload checks, run `rtk proxy node web/scripts/media-smoke.mjs`
after building the debug server; see [Media uploads](media-uploads.md). Its binary and
temporary data live under ignored `tmp/`.

`auto/debug.sh --with-s3` connects to an already-running local S3 service at
`127.0.0.1:19000`, with private bucket `commonroom-draft`, path-style addressing
and development credentials.
The launcher configures chat credentials and prepares the bucket/CORS settings.
Start S3 separately; debug never starts or stops it. With no flag it does no S3
setup or checks.
See [Development](development.md) for overrides and [Media uploads](media-uploads.md)
for independently deployed storage.

Media availability regressions hold local S3 requests open while checking that
ordinary chat reads still pass with a one-permit API budget and another file's
completion proceeds concurrently. Transport tests cover invalid/missing objects
versus 503 failures, zero-size guards, per-key batch deletion errors, in-flight
probe sharing without stale health caching, and filename encoding. Covering quota/partial expiry query plans, orphan triggers,
canonical SQL state, and write-free plain-text reconciliation use scratch data.

Media cache regressions cover reload reuse, live denial of cached bytes, canonical
playback/download URLs, native byte ranges, legacy CORS range headers, scoped worker
matching and disabled-storage fallback. Media engine tests cover restart claims,
shared references, last-reference removal, quota retention on failed deletion and
late staging recovery. Bot cache tests cover repeated renders/focus during offline
backoff, exponential rests, empty results and immediate notification refresh.

Object-command regressions cover scoped user lists, complete bulk metadata reads,
privacy, protected accounts, media inheritance/quotas, renames, room deletion,
read-only execution and failed-write rollback. Bulk grant/revoke tests exercise original-policy authorization, owners, protected su accounts, and the last active
administrator. Legacy upgrade fixtures are removed; unversioned databases and
versions 1–18 and representative future versions are tested for unchanged rejection. Upload hint tests
cover bounds/header injection, signing, replacement metadata and browser headers.

Command-output regressions cover full subcommand paths, permission-filtered help,
multiline manual usage, nested/numbered lists, tables, language-labelled fences,
literal errors/statuses, exact JSON and history/credential fence containment.
HTML and image loading remain disabled in command output. The isolated visual
fixture is `/scripts/commands/index.html` on the Vite development server, with
compact-width and dark-theme controls and no account/server data.

Command-interface regressions execute every catalogued form through the web
request dispatcher, including password hashing, bot credentials, all room/user
subcommands, and invitation targets named `ls`/`get`. The matrix fails when a
new catalogue form lacks a success probe. Separate owner tests verify implicit
room invitations, explicit `#room` targets, permission removal and public error
syntax; grant tests cover `#room` membership writes. Existing denial probes
cover every registered command without grants. Browser tests verify scoped
headings, one-level completion, free-form argument silence and bullet-only
manual explanations outside syntax/example fences.

Scope-default regressions verify current-room user lists and bulk metadata,
explicit global overrides, Console/stdin bot defaults, explicit `#room` cleanup,
and rejection of unsupported private defaults without global writes. Separate
grant/revoke tests preserve required direct-permission scopes and global group
defaults. Output heading tests use the originating view for omitted scopes.

Metadata-interface tests cover `@all` keys/users, effective 1g quota defaults,
account-wide shorthand, inherited media limits without global custom-key leaks,
case-insensitive k/m/g sizes, checked overflow, config serialization, and
readable metadata lists. Explicit bare room scopes are rejected across command
families; completion, manuals and integration fixtures use `#room`.


Cleanup regressions require explicit scopes in every view, exercise peer/pair and
bulk private targets, reject missing chats without creation, preserve private
scope privacy, enforce command/action/age checks during preview and execution,
and roll back batches on injected storage failures. Preview preserves database
write counts, audit and revisions. Media tests verify reference counts and shared
object retention. Metadata-authority tests cover admin/owner reads, all mutation
forms denied to non-su, explicit write-flag bypass rejection, and su writes.

Metadata tree regressions cover separate global/room value-and-source sections,
room override precedence, unset fallback, root-only quota, read-only resolution,
and filtering of global custom keys from room-only readers. Persisted room deletion
checks all account branches after restart, name reuse, preservation of other branches,
and rollback when an account update fails inside the deletion transaction.

Cleanup metadata regressions cover disabled users, live admin/owner defaults,
role removal and ownership transfer, global/room precedence, unset inheritance,
private cleanup, all-history requests, bulk denial atomicity, and sudo bypass of
command/action grants and limits without retaining elevated authority. Grant limit
arguments are rejected. HEAD persistence tests verify atomic metadata/grant
rollback; unsupported schemas are rejected without modification.

User reset regressions cover metadata key/all-key removal, inheritance fallback,
current-scope and account-quota defaults, bulk selection, Su-only writes, storage
rollback, branch pruning across restart, and preservation of password hashes.
Password reset uses the explicit `password` selector throughout console/web tests,
including authority rechecks after hashing, session revocation and UI redaction.
Global metadata entries have no hints; room entries retain inheritance/local hints.

## Static-review regressions

`engine/tests/review.rs` covers room-only metadata projection, explicit global
group grants and batch audit, membership invitation authority, uniform attachment
errors and edit ownership ordering, atomic first-DM reservations, no-op cleanup,
bulk key-cap prevalidation, resolved-key snapshots, and indexed batched cleanup
counts. Query-plan assertions inspect index selection without benchmarking.
Web tests cover portable Markdown attachment paths, complete Unicode hints, full
object cache bounds/recency/ranges, actual content lengths, and restart invalidation
of unsent attachments. Benchmarks remain explicitly opt-in and ignored by normal tests.

## Avatar coverage

The browser rename regression loads an avatar, changes the account name while
retaining the image URL, and verifies that the cached image becomes visible
without refreshing. New identities still cannot retain another name's frame.

Engine tests cover self-service URL validation, metadata isolation, allocation
limits, stable ownership, invalid MIME, rename/restart persistence, failed-write
rollback, expired uploads and account replacement. HTTP tests check session and
origin enforcement, rejected target injection, broadcasts and removal. Web tests
cover crop geometry, URL/base-path handling and optional snapshot validation.
The media smoke suite additionally uploads avatar bytes to real local RustFS,
checks unauthorized completion/download, immutable final objects, private signed
reads, restart cleanup protection, replacement and removal. Allocation replays check
stable IDs and reject different owners/sizes. Engine regressions cover visibility
revocation and disabled-account retention; HTTP info checks enforce session/origin
and live visibility. Query-plan tests require an uncorrelated cleanup reference list.
Web regressions cover retained image frames, identity changes, decorative/sized images,
default-off external URLs and live authorization on cached avatar bytes. Browser verification
should position/zoom a rectangular image, confirm, and inspect chat/sidebar/account
at desktop and compact widths. Check URL failures leave no blank avatar space.


## Chrome 114 compatibility

The production build targets Chrome 114 explicitly. `requestAbort.test.js` runs
both native and missing-`AbortSignal.any` paths, checking early cancellation,
first-abort reasons, deadline expiry during body reads, and listener/timer cleanup
on completion and failure. Build targets do not polyfill runtime APIs.

For browser verification, use an isolated Chrome for Testing 114 profile and
throwaway server data. Check the built UI at a non-root HTTPS base path: login,
room history, WebSocket sends and reload, desktop/compact settings, media config,
document previews, and request cancellation/timeouts. Also verify service-worker
activation and CSS `:has`, `color-mix`, and dynamic viewport units. Keep the installed
browser and working data folder untouched. No benchmarks are required.

Chrome 114.0.5735.133 was verified against the production bundle on 2026-10-22
using isolated temporary accounts and a `/commonroom/` HTTPS base path. Login,
history, WebSocket messages, reload, settings, Markdown/LaTeX, streamed document
preview, cancellation/deadlines, avatar canvas/WebP encoding and worker activation
passed at desktop 1440×1000 and compact 390×844 with no page errors. Storage was disabled; S3 transfers were not part of
this browser compatibility smoke. Unit tests cover the shared request path used
by avatar/media uploads and completion.

Login hardening regressions cover bounded tracking and expiry, pending reservations,
late completion across window rollover, account limits across rotating IPs, repeated
successful logins, independent login worker admission, malformed forwarding and
busy-worker refunds. Configuration tests reject unknown top-level and nested keys
without rewriting files and preserve unchanged formatting/mtime. Production ingress
tests require security headers on accepted, untrusted-peer and insecure requests.

On 2026-10-22, an isolated Chrome profile against the real Vite `web/` UI and
updated backend passed invalid-password and rate-limit display, login/socket
startup, room selection, Unicode message send, reload persistence, 390×844
layout without horizontal overflow, twelve successful logins, and logout. No
uncaught browser exceptions occurred. The five existing `test:browser` scenarios
also passed. The full HTTP/WebSocket smoke harness passed against rebuilt debug
and release binaries. Browser checks used temporary accounts/data with media
storage disabled.

Backend admission regressions cover socket eviction and pool isolation, stable
account HTTP limits, cancelled-request worker ownership, typed bot 429 responses,
shared transport shutdown and exponential login backoff across rotating IPs.
Large Rust suites live under `server/src/{engine,web,bots}/tests/`; private module
access and test-only compilation are preserved. Obsolete whole-store save helpers
and pointer-alias assertions were removed; persistence tests use captured deltas.
Concurrent same-key bot sends now deterministically replay the committed receipt;
unrelated stale preparations and revoked permissions still fail.

Media regressions verify one-second failed-probe backoff and recovery without
positive health caching. Download tickets recheck stable account identity, live
permissions, expiry and media SQL changes even when engine revision is unchanged.
History cache concurrency holds one shard’s page/unread locks while another
conversation completes both reads; shard and aggregate budgets remain bounded.

External avatar regressions cover denial by default, exact-host allowance,
suffix/subdomain/userinfo lookalikes and hiding previously allowed stored URLs
after policy changes. Direct bot ingress tests cover explicit opt-in, private
trust validation, default denial and rejection of forwarded HTTP even with opt-in.

Backend review regressions cover digest-based session lookup, bounded durable
authentication events, retained-row counters, typed retryable storage errors,
duplicate cookies across headers, and idempotent avatar broadcasts. Incremental
unreads are compared to an exact SQL oracle across global sequence gaps, cursor
advances, retention, retraction, cleanup and failed writes. Blocked-reader tests
verify both browser and bot snapshots release engine guards and discard frames
after permission or credential revocation. Transport tests cover bounded queued
coalescing, lag recovery, discovery/presence preservation and shutdown priority.

Metadata-template regressions compare reused browser/bot projections against cold
builds after sends and cursor advances, and verify invalidation after policy,
identity, directory, private-conversation and avatar-host changes and recovery.
Whole-frame tests prove unrelated room messages and another account's sessions
preserve the same shared buffer while visible content and revocation refresh it.
Cache resource tests bound profiles and conservative allocation accounting.
Resolved-key regressions check room/account selection, referenced metadata cleanup,
new-key rollback, password/session restoration and identity-index correctness.
Existing restart, bot-credential, rename, bulk-reset, room-removal and failed-write
tests run through the same keyed production commit path.

Socket batching regressions cover bounded eight-event worker chunks, mixed target
filtering, credential scopes, stale positive routing hints and oversized routing
fallback. Blocked-reader tests exercise conversation/unread batches and mixed
baseline/presence batches: writers revoke permissions, directory access or credentials
while reads wait, and every stale frame is discarded. Media smoke warms a recipient's
routing cache before silently reserving a private upload, then verifies that the
first private message still arrives without a baseline broadcast.


Metadata notification regressions compare changed viewers against independent cold
snapshots across room/account lifecycle and policy changes, cover old and new access,
and verify outsiders perform no history reads or baseline work. Transport tests union
coalesced audiences and retain bot command catalogue invalidations during forced
recovery. Bot credential API tests assert committed notifications. The real socket
harness checks hidden room creation, grants, revocation and regrant while an unrelated
viewer receives no baseline.


## Name color synchronization

Profile-budget HTTP tests require invalid palette, missing room, denied permission
and full metadata-scope attempts to leave all four valid writes available. They
cover 429 responses, no-op writes after exhaustion, unchanged state/version and no
broadcast on rejection, and the shared avatar allowance. Color-version tests cover
unrelated avatar, media, room creation and private-chat writes, actual color and
account lifecycle changes, failed persistence and explicit history opt-in.

Connection tests delay both exclusive commands and parallel messages across a
preference toggle, cancel a queued toggle and exercise real disconnects. Initial
snapshot tests cover saved opt-in, default-off and blocked storage; browser login
coverage checks the first authenticated snapshot. Picker tests compare six local
swatches before one Apply, discard Cancel/Escape/outside previews, preserve focus
during a slow save and avoid stealing focus after a rejected save.

Color regressions cover restricted readers without directory/member-list access,
authors first disclosed by live frames, authors outside snapshot tails reached by
history paging, reply authors and mentions, room inheritance and explicit resets,
private global-only colors, access revocation, and server restart versions. Web
coverage verifies nullable projection validation, bounded cache merges, separation
of display projections from saved room settings, metadata changes without message
revision changes, and stale in-flight history responses after a color reset.

Name colors, author borders and message side bars default off independently and
persist only explicit opt-ins. Browser regressions verify the original reply quote
bar remains when add-ons are off, Preferences loads the color picker only after
enabling display, socket/history color requests opt in explicitly, and disabling
display stops color requests. HTTP/live baseline tests cover default omission and
cache separation between opted-in and default clients on the same account.

Settings browser coverage additionally exercises the themed scope listbox,
macOS Safari-style pointer activation without native button focus,
arrow-key navigation, focus restoration, Escape without closing the parent dialog,
option/picker AutoAnimate transitions and live reduced-motion changes. Geometry
assertions require the message bar to span header, reply and body, with an inset
reply bar and no duplicate body bar. Palette tests check every name shade against
paper, wash and field surfaces, including mid-tone and saturated custom seeds.

Message bar opacity regressions cover the 50% default, saved 0–100% values, invalid
or unavailable storage and clamping. Browser checks exercise the live slider at
0%, 20%, 50% and 100%, inspect painted alpha, and preserve author text, reply quote
color and bar geometry while hiding the slider when bars are disabled.

Connected-bar regressions require adjacent same-author/minute segments to meet
without gaps or overlapping alpha, including reply headers, reaction rows and
message actions. Minute boundaries, author changes, private-pair changes, local
midnight and unread dividers split runs; retractions reconnect eligible neighbors.
Disabling bars preserves the original five-minute header grouping.

Room-only color tests verify grant rejection at global/account/private scopes, no
root-to-room authority inheritance, ordinary-admin revocation, cross-room and
cross-account isolation, bulk/protected-target atomicity, non-color metadata denial,
owner transfer, restart/rename/regrant behavior and failed-write rollback.
HEAD-only startup tests reject v17/v18 and all other unsupported schemas unchanged. HTTP tests reject set/reset/no-op room writes after
revocation without metadata/version changes or notifications, while root writes remain
available without global grants. Browser coverage keeps root settings available and
filters room choices using own/any grants, including live revocation fallback.
Completion covers the color key, full palette, @unset, bulk selectors and scopes.

## Server database refactor regressions

`engine/tests/storage.rs` checks database integrity, foreign keys, retained-row
counts and independently reconstructed account references after deterministic mixed
mutations, account rename/deletion and cleanup. A competing writer causes a bounded
unavailable result without partial memory changes; the next mutation succeeds after
the lock is released. Abrupt-exit tests spawn the test binary against an isolated
temporary database, kill it before/after an outer transaction commit, and verify
message, receipt, session, sequence and read cursor atomicity after reopening.

Attachment-free inserts/edits and identity rewrites are exercised with the reference
table deliberately absent, proving those paths do not query it. Media reference
change tests retain validation, rollback and shared-attachment coverage. The unread
SQL oracle includes edits, reactions and rename invalidation. Batch regressions
require one reader lease for cold unread requests and none for warm requests.
Serialized-size checks cover Unicode, escaping and reactions. Query-plan tests use
the same pagination SQL constants as production. Restart coverage combines cold
history caches, no startup writes and policy/session/cursor/history preservation.
Concurrency cases keep separate transport assertions while sharing their app and
reader-start fixtures. Mutation-cost checks compare work across unrelated history
growth and protect untouched tables instead of fixing send cost to trigger counts.

For server-only work, run `rtk cargo fmt --all --check`,
`rtk cargo clippy --all-targets --locked -- -D warnings` and
`rtk cargo test --locked`. These do not enter the optional `simpleBot/` worker.

## Expired attachment placeholders

Engine and HTTP regressions preserve expired metadata before/after object cleanup
only for current readers with retained references. Normal info/file requests stay
404 after expiry; `include_expired=true` reveals filename/expiry metadata only,
with session/origin checks and 404 after retraction. Media-cache tests ensure
expired metadata cannot serve cached full or range responses. Browser regressions
cover all attachment kinds, original Unicode filenames, light/dark surfaces,
live expiry and network failures that must not be labeled as expiry. Visual checks
include desktop and 320px compact layouts.

Native database contracts additionally verify transaction-assigned append cursors
and receipt sequences, counter rollback/exhaustion, PostgreSQL append commit
ordering and cancellation while waiting on the counter. Command scope tests prove
independent PostgreSQL handles commit outside an open command transaction and
that dropping a SQLite command scope rolls back and releases its single writer.
Pool shutdown must drain asynchronous lease returns before releasing database or
folder ownership; offline-transfer checks preserve source file bytes exactly.

Async snapshot-cache coverage cancels an active builder while another caller is
waiting, requires the waiter to fill the same entry, and verifies subsequent
callers reuse its shared buffer. Existing blocked-reader, credential-revocation,
partial-baseline and batch notification tests run through the native async
snapshot path; production synchronous plan wrappers have been removed.

Native application mutation tests block a PostgreSQL append at its sequence counter
and require an unrelated HTTP reaction and read acknowledgement to finish. They
cancel the original HTTP request, queue a metadata change, and verify committed
memory/receipt publication precedes that metadata change and replays exactly once.
Receipt-capacity tests race independent conversations at 4,095 existing receipts;
only one insert may commit and the rejected transaction must not advance sequence.
Conversation-coordinator tests cover cancellation cleanup and metadata exclusion.

Native browser text regression coverage preserves Send-only room access while
requiring Read before loading attachments. It verifies trimmed Markdown, filtered
mentions, committed notification delivery and rejection after Send revocation.
The PostgreSQL cancelled-append contract runs through both browser and bot entry
points and requires publication/broadcast after transport cancellation.

Native browser `/tell` tests inject a failed first-message insert and require no
private directory, cursor allocation or notification to survive. Opposite-direction
sends must share one directory and monotonic revisions; restart retains identity
and history. Existing-chat sends need no Direct permission while new chats do.
The PostgreSQL cancellation contract also covers uncommitted directory privacy,
independent-writer progress and metadata notification after a cancelled first DM.

The browser message-action contract runs on SQLite and PostgreSQL. It covers
edit authority, an editor's unreferenced upload added to another author's message,
retained attachments, stable author/sequence/Markdown, no-op edits, reaction toggles,
room/private reply quotes and global-superuser edit context. The blocked-append
application test requires unrelated browser edits and reactions to complete while
the append transaction remains locked.

An expired-retained-attachment regression requires native edits and reactions to
skip unchanged attachment lookups while preserving the message sequence and
reference. Newly introduced attachment references still undergo live validation.

Native history-deletion contracts run on SQLite and PostgreSQL: failed retraction
and multi-conversation cleanup must roll back revisions, messages and audit with
no notifications. Coverage includes author checks, reply scrubbing, attachment
references, preview/no-op/bulk denial, restart persistence, and exact 1,000-entry
audit retention. Stale-revision and bulk-lock/queued-metadata tests check atomicity
and lock ordering. PostgreSQL tests require audited deletions to progress during
a blocked append and sends to progress during a blocked cleanup; cancellation
retains cleanup commit, audit, publication and notifications before queued metadata.

Native attachment-media HTTP contracts run on SQLite and PostgreSQL with a
loopback object-store mock. They cover reservation/completion/read/download,
private-directory/quota atomicity, credential/permission rechecks, published
abandon protection and confirmed cleanup/staging grace. A PostgreSQL quota-lock
contract checks independent accounts, cancellation and queued revocation. A SQLite
WAL contract proves ordinary media reads use readers while command-scoped reads
retain uncommitted transaction visibility. Latest WIP verification and outstanding
real-binary/media checks are recorded in `database-refactor.md`.

PostgreSQL review regressions also cover cached read-health checks, actual
advisory-lock loss, single-statement page/unread SQL against the oracle, account
index query plans, rejection of old WIP schema markers, and rebuilding an empty
retention table past the runtime statement deadline after restore. SQLite
covers rejected COMMIT rollback without dispatcher poisoning. Shared tests verify
uploader-bound media updates, stale metadata CAS rejection, startup upload grace,
terminal avatar completion, sudo unwind cleanup, recognized expired logouts,
HTTP reservation reuse in spawned commands, and preserved login failure backoff.

The transfer contract requires exactly `SQLITE_SCHEMA_VERSION` as its read-only
source and verifies the independent `POSTGRES_SCHEMA_VERSION` on the target.
Non-HEAD SQLite versions fail before connecting to PostgreSQL or changing source
bytes. A deferred PostgreSQL profile trigger blocks inside COMMIT while its HTTP
request is cancelled; publication, subsequent authentication and another write
must succeed. A panic through `with_conversation` must poison the engine and
reject subsequent reads/mutations until restart, as documented in Security.
