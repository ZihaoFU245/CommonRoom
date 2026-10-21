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

Rust tests cover configuration validation, proxy trust, verified client-IP login throttling and spoofed/malformed forwarding headers, account permissions and deletion, room membership, private visibility, password/session behavior, retained history, reactions/replies/mentions, schema rejection, restart/move persistence, and failed-write rollback.

Web helper tests cover the shared request lifecycle (success, rejection, mismatched acknowledgements, send failure, disconnect and account promises), captured confirmations, private command routing, browser-local aliases, local command ordering, redaction, suggestions, Unicode mentions, notifications, browser timezones and daylight saving, fonts/storage failures, unread visibility, retention, and resynchronization races. Markdown tests cover browser-local HTML opt-in persistence, mixed HTML/Markdown, literal code, malformed markup and blocking active content, arbitrary attributes, unsafe links and images. Protocol/client tests exercise malformed nested JSON, frame variants, base-path URLs, credentials, abort signals, and API errors.

Build tests verify that both modes skip checks and that release packaging
contains the `dist/` directory with its files. Rust tests also verify that a poisoned engine
returns an unavailable response and never reuses potentially partial state.

`web/scripts/smoke.mjs` starts isolated debug/release processes on ephemeral ports and uses HTTP and real WebSockets. Every received frame passes the same runtime validator as the application. It checks authorization, commands, privacy, second-device directories, bounded room/private history, read cursors, account deletion, restart, API-only routing, base paths, trusted proxy/TLS headers, origin rejection, secure cookies, and logout disconnection. It never uses the working `data/` folder. Build both binaries first.

Current-schema persistence tests cover v17 restart and HEAD v16 upgrade, lazy history loading, state/cursor preservation, and rejection of every unsupported schema without modification. Object-command tests cover scoped listings, removed media-grant arguments, and group-removal authorization.

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

After changes to components/hooks, inspect both a wide desktop and compact view. Verify login, sidebar collapse and scrolling, room/private selection, local commands and hints, composing Unicode text, replies/reactions, settings, unread badges/jump, and reconnect behavior. Refresh must erase command output while restoring server chat history. Check browser errors and HTTP/WebSocket failures.

The isolated browser fixture at `/scripts/markdown/` on the Vite dev server
mounts 1,000 messages with counter, snake, safe HTML and isolation-check payloads.
Allow HTML and LaTeX start off. It does not access account data.

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
cleanup constraints, persistence failures, v17 restart and HEAD v16 upgrade and rejection of unsupported schemas.

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

`auto/debug.sh` supplies local RustFS connection defaults (`127.0.0.1:19000`,
private bucket `commonroom-draft`, path-style addressing and development credentials).
Explicit `CHAT_S3_*` variables override these defaults; `RUSTFS_ACCESS_KEY` and
`RUSTFS_SECRET_KEY` also supply matching local credentials when set. It starts
only chat and the UI. Start RustFS manually in another terminal with
`./auto/start-s3.sh`; stopping debug does not stop that independently started service.
Create the private bucket and CORS policy as described in [Media uploads](media-uploads.md).

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
administrator. Only HEAD v16-to-v17 migration is tested; pre-HEAD upgrade fixtures
are removed and versions 1–14 are tested for unchanged rejection. Upload hint tests
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
arguments are rejected. HEAD migration tests verify constraint transfer and atomic
rollback of metadata, grants and schema version.

User reset regressions cover metadata key/all-key removal, inheritance fallback,
current-scope and account-quota defaults, bulk selection, Su-only writes, storage
rollback, branch pruning across restart, and preservation of password hashes.
Password reset uses the explicit `password` selector throughout console/web tests,
including authority rechecks after hashing, session revocation and UI redaction.
Global metadata entries have no hints; room entries retain inheritance/local hints.
