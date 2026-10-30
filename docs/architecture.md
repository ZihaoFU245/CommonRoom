# Architecture

`config.json` explicitly selects `database.backend` (`sqlite` by default or
`postgres`). The PostgreSQL URL is supplied only through `CHAT_DATABASE_URL` and
never changes the selected backend by itself. Startup reads selection without
saving, acquires ownership, then normalizes configuration after rechecking that
the selection has not changed.
`engine/database/` owns SQLx sessions, native pools, typed operations, transaction
scopes, lossless JSON projections, backend schemas and offline transfer mechanics.
Each backend has independent SQL for messages, cursors, metadata, media, receipts
and history. SQLx owns prepared-statement caches. SQLite has one writer lease;
PostgreSQL has up to eight write leases and a separate ownership session.
PostgreSQL readers default to two through eight connections, with configurable
reader and writer limits; SQLite uses two to
four readers. PostgreSQL pages and unread batches use one coherent statement
snapshot. Shared history caches contain no SQL. Shared storage coordinates domain
metadata and publication through typed requests. Browser/bot history, snapshots
and socket notification plans await native readers. Metadata capture/revalidation
uses short engine guards, with database health checks outside those guards.
Browser plain-text/`/tell` messages and `/reply`, `/edit`, `/react`, `/retract`,
`/clean` actions, bot sends/reactions and browser/bot read acknowledgements also
await native writers. Plain browser text requires Send;
attachments additionally require Read before lookup. A conversation permit orders
preparation, database commit and memory publication. A new private directory and
its first message commit together; publication invalidates metadata caches and
emits a scoped directory notice only after commit. These permits share a metadata
fence; native metadata-changing plans acquire it exclusively before preparation
and retain it through commit/publication. Bulk cleanup counts before acquiring
conversation permits, then locks matching conversations in sorted order before the
fence. It revalidates the complete target set and every target revision, retrying
up to three times on change. Delete and audit remain one atomic transaction.
Audited deletions share a separate audit permit
through commit/publication; independent message/cursor writes do not acquire it.
Audit storage appends a single entry and prunes the oldest entries to retain 1,000.
Attachment media reservation, read, completion, abandonment, download revalidation
and background cleanup also await native operations outside engine guards. Media
reads use independent reader leases. First
private upload directories commit atomically with quota-checked reservations and
publish silently after commit. HTTP admission resolves credentials in memory as a
capacity hint; handlers retain authorization and storage-health checks. Avatar
allocation, completion, visibility and self-service avatar/color changes also use
native operations. Profile commits are ordered per account; PostgreSQL locks the
target account row and unrelated accounts can progress independently. Completion
workers retain the object mutex through database commit/publication even when the
HTTP caller is cancelled. Native read-only commands capture owned query plans;
history, cleanup previews and configuration cache statistics await readers outside
engine guards. Elevated query authority exists only during each synchronous
capture and is rechecked before delivery. Basic
account enable/disable, grant/revoke and room creation/deletion, membership
and ownership commands use owned native metadata deltas, including elevated
variants. They hold the metadata fence exclusively through commit/publication,
while releasing the engine guard for database work. Preparation restores committed
memory before returning its plan; failed writes cannot leak pending authority.
Bulk account metadata, room rename and credential commands also use owned native
deltas. Login/logout persist session changes and authentication events atomically;
bounded hash workers run outside engine guards. Browser socket/session checks await
health outside guards and revalidate stable identity. Presence is memory-only.
Account rename/deletion and slash-bot administration also use native transactions.
Rename awaits indexed references outside the guard under its metadata permit;
history, identity, sessions and cursors change atomically. Versioned bot
authentication, credential issuance, conversation creation, source facts, receipt
lookup, socket controls and command discovery use native async health/read/write
operations. Elevated browser message/history writes share native conversation
ordering and phase-local sudo authority. Bot command effects and retry outcomes
commit in one native transaction. Message/history commands use conversation
permits; metadata commands use the exclusive fence. PostgreSQL serializes receipt
checks per submitting account before acquiring append/conversation locks, while
unrelated commands can use independent writer leases. Console administration,
queries, global edits/retractions and cleanup also use native owned plans. Browser
fallback errors validate in memory; sudo local controls use the read-only query
path. Startup retention/media cleanup and shutdown checkpoints await native
storage. Accepted I/O and password-only login jobs drain through publication before
checkpointing. The stdin reader owns no database reference. Test-only command
adapters bridge to native preparation, persistence and publication; explicit
command sessions are test-only. Synchronous SQLx utilities handle bootstrap,
offline transfer and final pool draining. Backend
switching preserves single-process ownership and wire contracts. See
[PostgreSQL](postgresql.md) for runtime and offline transfer details. SQLite/WAL and
portable-folder details below apply to the SQLite backend.

Uncontended memory capture/publication uses short `try_read`/`try_write` guards
on the async task. Contention falls back to the blocking pool; no guard crosses
an await. Shared immutable history pages coalesce concurrent cold reads and
cache at most two encoded ranges per page. Viewer authority, cursors and unread
state are still validated separately. Media cleanup acknowledges successfully
deleted objects in one batch, retaining object locks through detached persistence.
Failed deletions remain claimed and quota-reserved for retry.

A known durable commit whose publication fails schedules one authoritative
metadata/cursor reload under the exclusive metadata fence. It rebuilds compiled
authority, indexes and cache clocks before notifying clients to resynchronize.
Reload I/O holds no engine guard, and shutdown drains queued recovery. Failed
reloads, unknown commit outcomes, lost ownership and poisoned engine locks remain
fail-closed and require restart.

CommonRoom has one Rust API/WebSocket executable, a separately built static UI served by Nginx, and a local configuration folder. SQLite keeps its portable database in that folder; PostgreSQL stores chat data externally. The web is a Preact application written in strict TypeScript. Node and pnpm are development/build dependencies, not production services.

## Server ownership

`server/src/main.rs` wires process lifecycle, configuration, shared application state, and graceful shutdown. `console.rs` owns the stdin superuser interface. `config.rs` loads and validates saved settings; settings require a restart.

`engine/mod.rs` defines the engine and module boundary. `Engine` coordinates domain changes through separate owners: `Store` holds the selected database
dispatcher, history readers, process lock, persisted cursors and pending maintenance;
`Presence` owns transient connections; `CompiledPolicy` owns compiled authorization;
and media state owns limits and avatar-host policy. Raw `Data` is private in production.
Transport adapters use stable account identity, password verification material and
credential lookup methods, and borrow a history handle for off-lock reads.
Regression fixtures retain test-only raw state access for rollback assertions.

Domain modules extend that coordinator rather than creating separate copies of application state:

| Module | Responsibility |
| --- | --- |
| `models.rs` | Persisted data and serialized snapshot/history types |
| `database/` | SQLx pools, typed database operations, backend SQL/schemas and offline transfer |
| `storage.rs` and `storage/` | Database lifetime and committed-state coordination for bootstrap, entities, cursors, accounts, media, receipts, authentication and maintenance |
| `history.rs` and `history/` | Typed pooled database reads and statistics; focused keyset pagination, immutable pages, bounded caches and batched unread projections |
| `authorization.rs` and `authorization/` | Policy types, predefined bundles and compiled index; `access.rs` owns access checks/delegation, `commands.rs` owns policy inspection/mutations |
| `objects.rs` | Object command listing, scoped account metadata, bulk metadata selection and validation |
| `accounts.rs` | Account/session lifecycle, roles, provisioning, and account deletion |
| `rooms.rs` | Room creation, deletion, invitations, joining, and leaving |
| `messages.rs` | Owned transient message mutations, reactions and replies; room/private metadata contains no message bodies |
| `cleanup.rs` | Explicit history cleanup scopes, authorization and read-only previews |
| `queries.rs` | Snapshot authorization, visible-conversation and directory helpers; history/read queries |
| `read_model.rs` | Metadata epoch, bounded browser/bot templates and viewer-specific snapshot stamps |
| `notices.rs` | Committed metadata audiences using stable principals, resolved keys and old/new access |
| `snapshot.rs` | Owned browser/bot plans; history loading, unread collection and snapshot assembly |
| `presence.rs` | Connection counts and shared active-directory/frame projections |
| `dispatch.rs` | Input validation, read-command planning, scoped command context and test adapters to native mutation plans |
| `helpers.rs` | Password hashing, names, timestamps, mention scanning, and private-pair keys |
| `tests/` | Engine regression tests using isolated storage |

`server/src/bots/` owns the machine profile, credential management and
transient activity and command discovery. `bots/control.rs` owns the native command and verified source-context HTTP adapters.
`bots/discovery.rs` owns
the bounded socket-lifetime catalogue registry and correlated requests; browser
metadata reads recheck current engine permissions and publisher credentials.
Bot-only account/credential/conversation indexes rebuild
after metadata commits; new private chats add their index entry on commit.
`bots/socket.rs` caches credential context and rechecks live expiry/revocation without
rehashing credentials. Read/reaction preflight reads release the engine lock and
recheck conversation revisions before committing. `bots/engine.rs` is an engine extension for typed chat
operations using stable conversation IDs. Browser and bot adapters share message
insertion, reply quotes, action authorization and reaction mutation in
`engine/messages.rs`. Owned write plans in `engine/command_write.rs` and typed
backend operations coordinate durable message changes; read acknowledgements use
owned cursor plans. The bot adapter owns v1-specific
validation, retry receipts and error projection. SQL belongs to the independent backend modules under `engine/database/`.
HTTP writes and compact notification/discovery sockets are separate from browser
commands/snapshots. Workers configure their own settings through advertised chat
commands and enforce their own owner/admin policy. Registrations and receipts use
stable ordinary account IDs. `web/scripts/bot/` is a reference worker with its own
checkpoint/outbox, not a server-side agent runtime.

`commands.rs` outside the engine owns help/completion metadata and the shared account-provisioning syntax used by stdin and web password jobs. `manual.rs` provides read-only command and grant-system manuals. The domain dispatcher executes commands; the registry does not authorize them.

`web/mod.rs` owns route construction and `App`. Its child modules separate proxy/origin security, authentication, history/read HTTP handlers, and WebSocket transport. `web/commands.rs` routes commands and configuration output; `credential_commands.rs` hashes passwords on blocking workers, then commits through native credential plans. Identity and verification material are rechecked before commit. Backend SQL belongs in `engine/database/`.

HTTP admission and browser/bot authentication share a typed principal lifecycle.
Request-local reuse saves the admission capture without granting authority:
health and final live credential checks still run. `admission::spawn_io` carries
that context and the accepted job's permits through cancellation. One command
execution classifier is shared by adapters, and the public command reference is
generated from the same catalogue used by help/manual output. Test administrative
execution and avatar setters bridge to production pending operations rather than
implementing an independent metadata persistence path.

One `Arc<RwLock<Engine>>` permits simultaneous engine readers and serializes short memory mutations. Native browser message/history mutations, bot sends/reactions and read acknowledgements release it for SQLx I/O, keeping a per-conversation permit through publication. Different conversations share the metadata fence and can write concurrently. Administrative plans acquire the fence exclusively through native commit/publication. Short memory phases try the engine guard on the async task and use blocking workers on contention; live database reads and writes await SQLx outside engine guards. Synchronous command adapters and lock helpers are test-only. HTTP work admission is capped at 64 jobs and eight requests/commands per stable account. Account leases survive cancellation until accepted async or blocking work ends. Login uses a separate two-worker password pool and one blocking job per admitted request; its bounded IP/account failure windows expire incrementally rather than scanning all tracked clients. Never hold an engine guard across `.await`. HTTP history captures authorized resource identity/revision, releases the engine guard for native database I/O, then rechecks session, access and identity before returning a coherent revisioned page. SQLite WAL has one writer and a pool of two to four read-only connections. The standard OS RwLock does not promise a particular scheduling/fairness policy. Changes are broadcast after successful persistence; read-only commands do not rewrite state. Account deletion commits message/session changes and cursor cleanup together. In-memory rollback/cursor updates must retain this atomic behavior.

## Web ownership

`web/src/main.tsx` boots the app and selects login or chat. `components/Chat.tsx` composes the screen and connects feature hooks; individual components render navigation, messages, composer, local output, login, and settings.

- `api/protocol.ts` declares wire types and validates untrusted JSON. `api/client.ts` owns relative URLs, cookie-based requests, response validation, and outbound frames.
- `hooks/useConnection.ts` owns WebSocket lifetime, reconnect backoff, current snapshot, and HTTP resynchronization. Callback refs avoid stale state without reopening sockets on every render. Resume events share a 100ms debounce; reconnecting sockets supply their baseline without an extra resume HTTP snapshot. Hidden pages, disconnects and cleanup cancel queued resume work.
- `hooks/useHistory.ts` loads 100-message pages and keeps sliding conversation windows, rejecting stale responses.
- `features/conversation/historyCache.ts` bounds each window to 1,000 messages/approximately 4 MiB and the shared browser history cache to 5,000 messages/16 MiB/32 views. Oldest views are evicted.
- `hooks/useVirtualTimeline.ts` mounts the viewport plus eight rows of overscan, measures variable row heights and preserves a message anchor when pages move. Scroll position stays in a ref; scrolling schedules a render only when the mounted window or viewport height changes. Size/entry changes still recompute the window using the latest position. History anchoring captures the scroll coordinate before DOM commit, so a shorter replacement cannot use the browser's already-clamped offset as the old anchor. Resize callbacks reuse border-box observer sizes, with geometry fallbacks when unavailable; synchronous mount/edit and painted-position measurements remain explicit.
- `hooks/useReadTracking.ts` owns visible-message acknowledgements, scroll placement, and jump-to-unread.
- `hooks/useCommands.ts` coordinates aliases, reactions/retractions, output and navigation. `hooks/useCommandComposer.ts` owns drafts, completion, keyboard controls and reply selection. `features/conversation/requestLifecycle.ts` owns exclusive commands, bounded overlapping ordinary sends and completion on send failure/disconnect; `submission.ts` captures confirmed command targets and constructs frames.
- History loads automatically in both directions in pages of up to 100 messages,
  retaining the reading anchor without manual paging controls or loading notices.
  First entry after a page load starts at the first unread message (or the bottom
  when read); switching conversations restores the in-memory position. Jump to
  unread offers one fixed destination per unread batch and disappears after use;
  read acknowledgements cannot move that destination or reoffer it. A new jump is
  available after the batch clears and later unread messages arrive. Jump to bottom
  appears three viewports away and stays until reaching bottom, independent of speed.
  A transparent, zero-height overlay holds one floating jump button at the top
  right of history, preferring unread over bottom. It has no unread count and only
  the button captures clicks; messages remain clickable through the empty area.
  A dedicated AutoAnimate container slides a stable notice node over the shared
  120 ms ease-out timing, without fading, scaling,
  animating its height or virtual rows, avoiding repeated viewport resizes during
  scrolling. `features/conversation/timelineScrollMotion.ts` retargets 300 ms jumps as
  rows are measured; user input cancels motion and reduced motion skips it.
  Programmatic scrolling releases bottom following and does not trigger lookahead.
- `hooks/useLocalConsole.ts` owns the tab-local command ledger and clearing. It never persists command output.
- `hooks/useFonts.ts` and `useComposerResize.ts` own presentation preferences and input sizing.
- `CollapsibleMessage.tsx` caps long rendered message bodies at a 256px top preview with small borderless Show more / Show less controls and directional SVG arrows, including fenced code and plain text. `useMessageExpansionMotion.ts` starts the height animation before changing expansion state. `messageExpansion.ts` gives very long messages 420ms, spending 80% of that time revealing the visible viewport portion before completing the offscreen height; short expansions and collapses use 300ms. Collapse pins the visible footer so text compacts from above while following messages stay fixed; manual input releases that anchor. Virtual timeline anchoring yields while a panel carries `data-message-resizing`, keeping its measurements current. Reduced motion changes height without animating. Natural-content ResizeObserver measurements handle delayed Markdown/media and width changes. MessageList retains expansion within the current conversation window across virtual row remounts, clearing it on edits, eviction or conversation switches. Editors remain fully visible.
- `hooks/useMessageEditorLayout.ts` owns inline editor sizing, focus, animation, and last-message scroll anchoring; `ConsoleMessage.tsx` owns rendering and edit interactions.
- `hooks/useContentResizeMotion.ts` animates delayed message-body size changes (images, video metadata, lazy Markdown/math, attachment status and HTML/frame previews) with the timeline's 180ms ease-out timing. It observes an inner natural wrapper and animates the outer wrapper, including the fold control, without resize feedback. Bottom-following transitions temporarily own scroll placement and let virtual measurements update without competing anchoring; manual input releases the pin. Reading older messages retains the virtual message anchor. Explicit Show more / Show less motion takes precedence; width changes, initial mounts, offscreen content and reduced motion do not replay the transition. Interrupted loads retarget from the painted height. HTML covers use this common message transition instead of a second nested height animation.
- `hooks/useTimelineMotion.ts` animates live message arrivals, retractions and edits with transform/opacity effects on mounted rows. It preserves both row and text positions when grouping or date headers change, fading incoming/outgoing headers. It waits for synchronous virtual measurements before painting, shares the editor's 180 ms ease-out timing, and keeps temporary inert exit copies outside virtual sizing and read tracking. Short histories temporarily extend the paint clip into unused viewport space without adding scroll range. Conversation identity, history readiness and revision changes prevent animations during initial loading, paging and view switches. Animation frames stop when the transition finishes; scrolling updates cached positions without geometry reads.
- `useHistory` distinguishes background revision refreshes from initial loading and explicit paging; refreshing an existing window does not insert a loading bar or move the conversation viewport.
  A jump target is cleared after its page commits, so later refreshes follow the
  visible anchor and live arrivals can resume merging into the loaded window.
- `features/conversation/` contains pure ordering, date, mention, and unread helpers. `features/preferences/` contains account preference synchronization and notification helpers.
- `features/conversation/math.ts` renders untrusted LaTeX with bounded KaTeX settings. `components/Math.tsx` rebuilds that generated markup as Preact nodes and CSSOM styles under the chat CSP; message-supplied HTML never uses this trusted path. The Markdown token converter creates math nodes independently of HTML; generated nodes survive safe HTML filtering. Preferences shares the `/markdown` sending state and stores the separate LaTeX display preference locally.
- Markdown sending preferences/manuals live in the dependency-free `features/preferences/markdown.ts`. The message renderer uses a shared async loader without suspending virtual rows: plain-text chats do not load Markdown-it. Basic Markdown images use validated URLs and passive nodes. TeX tokenization and KaTeX are separate optional chunks; Security options and math default off.
- Account shows the profile card and an inline username editor. Security owns the expandable password editor and the account-wide Allow HTML option, defaulting off. Account editors animate with auto-animate, clear drafts on cancellation or section changes, and close after successful commands. Every HTML portion requires a Render HTML click and uses the same isolated `MarkdownFrame.tsx` preview, including passive HTML. Mixed messages render surrounding Markdown normally; `htmlFragments.ts` extracts HTML with token positions and balanced tag boundaries, retaining inline formatting, math and literal code. Adjacent HTML/CSS/script portions share a viewer. Each HTML portion starts as a compact Show text / Render HTML cover. Show text toggles a scrollable source panel capped at 400px; both panels use the shared message-content height animation and scroll anchoring. With Allow HTML off, `HtmlSource.tsx` displays the same bounded source panel directly. Reduced motion skips the animation. Full HTML documents bypass Markdown indentation parsing; fenced/code-span examples remain literal.
- `HtmlMarkdownGate.tsx` loads the optional HTML source converter on demand and retains source on failures. `richMarkdown.ts` converts mixed Markdown/HTML for the frame; the sanitizer remains a pure helper rather than a separate display path. `MessageList` bounds independent fragment choices to the current history window, resets all fragment choices on edits/settings/conversation changes and memoizes state across scrolling. ResizeObserver keeps virtual heights current.
- `markdown-frame.html` is a single generated static resource with an inline bootstrap compiled from `markdownFrame.ts`. Vite exposes its content-versioned URL, and Nginx serves it with a dedicated CSP and immutable caching. It has no bundle/font/CSS subrequests. Each selected preview is an iframe with only `sandbox="allow-scripts"`; same-origin access, forms, popups and top navigation remain disabled. A one-shot parent message supplies the source; the frame applies a leading CSP again before executing the HTML. It reports bounded heights via source-checked messages and ResizeObserver, without polling. Frames unmount outside the virtual window; remounts use the browser's cached shell and restart the selected preview. No hidden frame pool or persistent game state is kept.
- The frame allows inline CSS/JavaScript and data images/media/fonts, but blocks fetches, external resources, nested frames, base URLs and form actions. The app's leading `frame-src 'self'` meta policy also applies without Nginx; the frame carries its own leading CSP meta. Production headers add frame-ancestor restrictions and preserve a strict script/style policy on the main app. Full HTML code remains untrusted and never runs with the chat's origin.

Server snapshots are authoritative for membership, scoped permissions and commands, peer directories, and messages. The legacy wire `admin` flag is informational; UI controls use effective grants. Presentation, formatting, Security and notification choices persist in the account metadata tree and synchronize across authenticated devices. Browser storage keeps an account/deployment-scoped settings cache, legacy settings for first-login import, browser-local command aliases, and a bounded cache of downloaded immutable media bytes. Notification permission itself remains browser-local. Stable event callbacks and memoized message lists keep draft edits from sorting and rendering the transcript again. Keep pure helpers independent of browser APIs where possible.

## Scaling and extension

Snapshots log and omit individual unavailable history tails or unread entries
while retaining healthy conversation data. Missing unread data is not reported
as zero unread. Poisoned history-store locks fail closed with a restart diagnostic;
HTTP snapshots return 503 and browser sockets send an error frame before closing.

This layout scales feature development by putting each change in its owning module. Preserve the single state owner and add modules when a responsibility becomes distinct. Extra crates, generic repositories, or a global frontend state framework are not required to add ordinary features.

`max_messages` is a disk retention limit per room/private pair (default 1,000), independent of cache capacity. Raising retention does not raise cache budgets. Startup loads entities, policy, sessions and read cursors, not retained message bodies. SQLite HEAD is v20; v19 upgrades atomically to explicit bot credential kinds. Unpublished PostgreSQL uses its independent v1 schema and has no historical WIP upgrade path. HEAD SQLite always transfers read-only into current PostgreSQL. Deletion retention aggregates once per statement/conversation. Unsupported schema markers are rejected unchanged.

The server creates cache pages lazily: at most 1,000 cached message entries per conversation, 256 pages, and default global budgets of 10,000 entries and 32 MiB of conservative payload accounting. `database.history_cache` tunes the global byte/message budgets at startup. These are cache budgets, not a promise about total process RSS; response copies, SQLite caches, metadata and temporary maintenance work also use memory. Unread results have a separate bounded 4,096-entry revision/cursor cache. Both caches use four conversation-hashed mutex shards; each shard receives one quarter of the global message, byte, page and unread-entry budgets. One hot shard cannot borrow another shard’s allowance. Recovery retains configured limits. Grant-authorized `/system configs` shows configuration and `/system metrics` shows lifetime cache counters, hit rates, shard occupancy and pool acquisition statistics as Markdown. Metrics use existing cache locks/counters without periodic profiling or logging; see [Cache metrics](cache.md).

Committed native appends refresh cached tails after publication; edits/reactions rebuild affected immutable pages, carry unaffected pages and retain valid unread entries. Covered bounded history intervals reuse shared page slices. Metadata/deletion revisions keep conservative invalidation. Failed commits never refresh caches.

History uses indexes for stable message IDs, authors, timestamps, global maintenance sequence and conversation IDs. Frequent mutations use a small transient working set and targeted transactions; administrative rollback captures resolved keys, omits message bodies, and uses explicit mutations instead of whole-state comparison. Persistence keys are selected explicitly, without pointer identity; every metadata write requires a captured baseline and there is no full-rewrite fallback. Read commands share one fast path across browser, stdin and sudo. Account rename/deletion uses a reverse name index and 100-row batches inside one transaction; rename revises only conversations with matching message/reaction references. Cleanup/retraction and retention pruning execute in the selected database, so evicting a cache page never deletes disk history.

New sends/replies/reactions broadcast a `conversation` update only to authorized readers of that conversation. New private directories and policy changes use a full snapshot. Presence changes send only the authorized online directory, and extra tabs do not broadcast unchanged presence. Snapshot tails are globally capped at 1,000 messages/4 MiB serialized payload and at most 50 per conversation; the client independently bounds tail memory. Lagged subscribers receive a baseline snapshot. Full snapshots and uncached unread calculations still have costs proportional to authorized conversations/retained sequence ranges.

History uses stable sequence cursors, not deep OFFSET pagination. Requesting a range near sequence 1,000 reads that page directly without reading the first 999 messages. `around` probes at most half a page backwards before reading the page. Browsing beyond a 1,000-entry window evicts the opposite end; returning to it reloads only that page. See [Data](data.md) and [Protocol](protocol.md).

This release is a single-process service. An OS lock enforces one local data-folder owner for either backend; a session advisory lock additionally enforces one PostgreSQL database owner. Ownership precedes configuration normalization. Horizontal scaling still requires shared metadata/presence coordination and cross-process event delivery. A reverse proxy/load balancer currently supplies TLS and routes to that single process.

Socket baselines share serialized buffers per account and browser/bot profile.
The cache is bounded to 128 entries/32 MiB and keyed by mutation revision,
visible presence revision, and the account's read revision. Browser HTTP/socket
baselines share buffers; bot v1 HTTP/socket keys also include the credential ID.
Read-only commands and unrelated read cursors preserve baselines. Every caller
rechecks its credential before reuse. Concurrent readers await one baseline build per key through an async cell. A cancelled builder releases waiters so another caller can fill the entry; partial baselines are not retained. History responses
share immutable page slices and exact per-message wire sizes, so cache hits and
snapshot byte budgets do not copy or serialize message bodies again. Unread
lookup uses a hash index with bounded ordered recency tracking. Command lists
filter a shared catalogue using one effective-rights calculation per scope.
Browser cache entries retain byte counts and global running totals; ordered page
merges and directional trimming are linear. Ledger ingestion reuses its ID set
and skips sorting when no new message IDs arrive.

`engine/storage/command_receipts.rs` owns atomic native command outcomes. An explicit command-scoped handle keeps ordinary command savepoints and its
receipt on one connection; independent handles use their own write leases.
Receipt failure rolls back the command and reloads authoritative metadata/cursors
before broadcasting. The receipt wrapper does not clone metadata/cursors on success; administrative commands inside it retain snapshots of their resolved metadata keys.

Browser API failures retain HTTP status; 401 responses return the user to login.
Requests have a 15-second client deadline. Login sessions are capped at eight per
account with oldest-expiry eviction; the instance ceiling scales with max_users.
History retries preserve range/direction and stop after three attempts. A coherent
received page remains usable during live revision churn, with a refresh status.
Live snapshot tails enter the ledger even when paging fails. Read acknowledgements
use one pending timer reading current state, so incoming messages cannot starve it.

Audit entries are stored separately from policy JSON. Metadata saves write
only changed entities; retraction and cleanup use targeted message/revision/audit
transactions with rollback recovery. History pages batch reactions in one range
query. Unread counts/first/bounds come from one SQL statement, seeking only the
unread sequence range and indexed retention edges. Snapshot construction reuses
one effective-rights result per scope. Bot sends prepare receipt/reply lookups
under a reader guard, then recheck revision, grants and receipt under the writer.

Virtual rows retain their ResizeObserver subscriptions until removal; unchanged
viewport coordinates do not trigger state updates. Message rows, date formatting,
composer suggestions and visible history arrays are memoized. Motion captures
geometry on layout/transition changes and derives interrupted animation offsets
from animation timing, without a per-animation-frame measurement loop.

If rollback recovery cannot reload or validate authoritative metadata, engine
access fails closed with a restart diagnostic; partially mutated memory is not
reused by HTTP, socket or console callers.

Bot command completion uses a session-local 32-conversation/512 KiB cache and
four background reads, prioritizing the selected conversation. Failed discovery
backs off from 30 seconds to five minutes; focus refreshes rest for one minute
(five minutes for empty catalogues). Explicit catalogue notifications and membership
changes bypass those rests. Policy/account/
server-instance changes replace the cache. Server discovery preserves eligible
metadata during routine refresh, but explicit worker invalidation clears it.
Transient bot activity may include provisional plain text; it travels through a
bounded broadcast and is reauthorized for each browser recipient, independently
of persistent chat and its unread/history synchronization.

History selection reads its bounded cache synchronously, reconciles known tail/
retention facts for display, and refreshes in the background. A complete snapshot
tail is already a coherent history page. Read tracking keeps 32 stable-identity
scroll anchors with pixel offsets; the virtual timeline exposes capture/restore
operations. `historyLookahead.ts` predicts a bounded paging margin from scrolling
velocity and measured HTTP latency. Composer sizing shares one animation-frame
measurement and reuses computed styles until layout/font changes.

## Media uploads

`web/media.rs` owns browser upload/config/completion/download HTTP adapters and
background cleanup orchestration. `engine/media.rs` owns scoped authorization,
scoped user metadata limits and configured quotas. `engine/media_store.rs` and
owned media plans coordinate typed lifecycle operations; independent SQLite and
PostgreSQL modules under `engine/database/` own media SQL, transactional message
references, deletion claims and SQLite schema v20. `media.rs`
provides the S3 transport through rust-s3. Storage network calls run outside engine
guards. Finalization/cleanup serialize per object, with permissions rechecked before
publication. `useMediaUploads.ts` owns browser progress, cancellation and safe
attachment preview state and acknowledgement-based publication; `api/media.ts` validates responses and direct-upload transport.
Startup claims unreferenced tracked uploads before serving requests; cleanup sweeps
immediately and daily by default, with short retries for incomplete work. Quota
remains reserved until confirmed storage deletion. `mediaWorker.ts` intercepts only
application media and managed avatar file GETs and uses `features/conversation/mediaCache.ts` for a
128-entry/256 MiB persistent byte cache with live metadata authorization and range
support. Other API/static requests follow their normal network paths.
See [Media uploads](media-uploads.md).

Attachment playback/previews live in the lazy `components/Attachment.tsx` chunk.
The Markdown renderer recognizes only local media link hints; clicking Play or
Preview validates authoritative metadata before loading bytes. Browser codecs
handle playback; file signature classification uses a bounded 4 KiB S3 range
without loading whole files or adding decoder libraries. HTML/SVG file previews
reuse the existing isolated frame and opt-in policy rather than another renderer.

Command replies share `ConsoleOutput.tsx` and the lazy `Manual.tsx` renderer.
`features/conversation/manual.ts` formats output for display without changing
wire payloads; `commandMarkdown.ts` tokenizes full Markdown with HTML and images
disabled. The renderer creates Preact nodes and safe links directly, without
HTML insertion, attachment loading or message-preview preferences. `/help` and
manual usage derive complete invocations from the same server command catalogue.

`engine/cleanup.rs` resolves explicit cleanup scopes, authorizes every target,
and prepares counts before scheduling history deletion. Read-only previews use
the same plan under the shared engine lock; execution commits through existing
history-maintenance transactions and attachment reference triggers.

Account metadata uses a two-level override tree implemented with ordered maps:
`@global` is the root and stable room IDs key sparse room branches. Resolution
prefers a room entry, then its global entry, then a known media configuration
default. Cleanup limits instead fall back to live admin/owner/user role defaults.
Only explicit values are stored. `media.user_quota` is root-only.
Reserved `preferences.*` keys are global-only, validated against a fixed key set,
and separate from the 32-key allowance for other metadata. Login atomically fills
missing preference keys from that device; existing server values win. The
self-service writer accepts only the authenticated account. Generic metadata
set/reset commands reject preference keys even for su and the console, while
whole-root resets preserve them. The writer uses the exclusive metadata fence and owned native commit/
publication, notifying only the account whose preferences changed.
Command read projections carry per-scope values and their source, filtered by
account/room read authority. Room deletion prunes every account's matching branch
inside the existing room-deletion transaction; a failure rolls back both.

Cleanup previews batch up to 256 authorized conversation IDs per aggregate query on
one history reader, using the existing conversation/timestamp expression index.
A cleanup with no matching messages leaves the audit, database and revision unchanged.

`engine/avatars.rs` owns self-service profile validation, ownership and the avatar
metadata projection. `web/avatars.rs` adapts authenticated HTTP requests to that
engine and the shared S3 transport. Avatar files use the existing tracked media
lifecycle with a dedicated `avatars/` final prefix and profile references instead
of message references. `AvatarSettings.tsx` owns the local crop/confirm flow;
`Avatar.tsx` keeps the last loaded frame visible while a replacement loads, uses
explicit image dimensions and decorative alt text next to names, and hides external
URLs until the viewer opts in through Security. Snapshot projection validates only
visible users' avatars. Managed reads resolve the owner through the existing stable
account ID index, check its current reference and viewer visibility, and recheck
after signing. The existing media worker caches avatar bytes with live `/info`
authorization. Crop controls draw from a 1024-pixel working canvas once per animation
frame; a local blob preview covers upload and canonical image loading. Account avatar
state stays mounted across Settings tabs.

Media-cache recency rewrites are limited to once per object per minute within a
worker lifetime, with a bounded in-memory timestamp map. Persisted eviction order
is approximate LRU; every byte reuse still checks live `/info` authorization.
Serialized cache maintenance builds an ordered size index lazily once per worker,
then maintains its byte total on insertion, deletion and recency rewrites instead
of scanning every cached response on each insertion. Before insertion it reconciles
the live key list, reading sizes only for unfamiliar keys, so overlapping worker
lifetimes and browser eviction cannot leave stale accounting indefinitely. Storage
failures discard the index for rebuilding on the next write.

Bottom-following intent survives temporary scroll-height growth from animated
rows and delayed content. Wheel, touch, pointer and keyboard input cancel timeline
motion before scroll tracking evaluates the new position; readers of older
messages are never repinned just because another row is animating.

Browser and bot sockets share transport lifecycle, heartbeat, idle timeout and
frame-budget helpers. Stable-account admission allows four sockets per profile
and cancels the oldest on replacement. Separate browser/bot pools hold 128/64
leases. Shutdown wakes every transport before checkpointing. Bot control frames
use the same account-wide budget as HTTP writes, except correlated discovery
responses to bounded server-owned requests.

Browser and bot baselines capture an owned authorized plan while holding an engine
read guard. History reads, unread queries, response assembly and serialization run
after releasing it. Before delivery, transports recheck live credentials and the
captured revision key, retrying at most three times. A stalled WAL reader therefore
cannot block account revocation. Snapshot caches share complete frames by profile
and revision; incomplete/error builds remain retryable.

Unread caches update exact counts from append/cursor deltas when the conversation
revision difference equals the number of new retained rows and retained bounds
remain compatible. Edits, reactions, retraction, cleanup and retention eviction
fall back to an exact indexed query. Global sequence gaps do not imply message
counts. `unread_full` and `unread_incremental` expose these paths in history stats.

Presence owns transient connection counts and shares the active directory and
serialized frame until account/connection state changes. Socket event queues drain
at most 64 changes per batch, coalescing repeated invalidations and recovering lag
with an authorized baseline. Presence and command discovery survive baseline
coalescing. Logout/password notifications target the affected account; unchanged
avatar writes perform no persistence or broadcast.

Metadata and content use separate invalidation domains. Administrative commits,
identity changes, avatar-host policy changes and recovery advance the metadata
epoch. Profile-only commits advance delivery epochs and refresh bounded cached
avatar values while preserving compiled authority templates. Browser/bot metadata templates hold only authorized directories/access and
owned query descriptors, never retained history. They share a 128-profile/32 MiB
conservative allocation budget; oversized templates are rebuilt rather than cached.
Ordinary sends, reactions, cleanup, sessions, presence and read acknowledgements
refresh dynamic revisions/cursors without rebuilding metadata. Serialized baseline
keys include the metadata epoch, exact visible conversation IDs/revisions, the
viewer's read revision and authorized presence revision. Unrelated history/session
activity therefore preserves the shared frame. Every caller still rechecks live
credentials and the complete stamp before delivery. History pages remain shared by
stable conversation ID, content revision and range across viewers.

Administrative rollback and persistence carry explicit resolved keys for account,
session, room, private-conversation and bot-registration maps. Commands validate
bulk selections before mutation. Creation/rename includes missing destination keys;
rollback removes new keys and restores selected originals without replacing unrelated
maps. Room removal selects only account metadata branches referencing that room.
Account rename expands keys through indexed message references; account deletion
intentionally touches every conversation revision for anonymization. Bot commands
use the same keyed commit instead of cloning all Data. Full-data delta fixtures are
test-only. Policy/audit serialization retains its existing transactional layout.

Engine domain, authorization, command, persistence and history operations retain
`EngineError` classes. SQLite/filesystem/serialization failures are sanitized at
source; transports map classes centrally instead of guessing a status from the
operation. The history pager retains its checked range/revision error type and
converts it explicitly. Native bot commands preserve failure classes through their
receipt transaction. Browser command frames and console output format errors only
at the final text boundary. Hidden policy/catalogue resources return the same
failure class for missing and inaccessible targets.

Avatar changes broadcast a stable-account invalidation and render compact profile
frames. Owned visibility plans capture allowed conversation IDs; historical-author
visibility uses one indexed reader query outside the engine guard. Delivery rechecks
live credentials and the engine revision, retrying at most three times. It does not
load message tails or unread counts. Frames coalesce by account, and full baselines
subsume them. Browser updates preserve all existing history references; bots skip
profile invalidations. A bounded, independent per-account avatar mutation budget
limits actual changes while preserving idempotent replay.

Browser and bot sockets render coalesced notifications in batches of at most eight
per notification worker. Browser capture authenticates once and owns conversation,
unread and avatar plans; database reads and serialization run after releasing the
engine guard. Final delivery checks the live identity, metadata epoch, viewer read
revision, relevant conversation revisions and presence when needed. Bot compact
notifications share one credential/scope check and guard per batch; full baselines
retain off-lock reads and final credential/stamp validation for every frame.

Each socket keeps at most 4096 readable conversation IDs as negative routing hints.
Unrelated conversation/discovery/activity events skip worker admission. Hints never
authorize a delivery. A read-only shared metadata clock disables stale exclusions
immediately, including private conversations created silently by media reservation.
The next worker refreshes the hints under live authorization. Baselines also refresh
them; oversized projections fall back to live routing rather than growing the cache.
Read acknowledgements coalesce per viewer, and full baselines subsume content updates
while preserving presence and discovery. Heartbeat rename accounting runs in its
blocking worker. All locks are released before socket sends.

Metadata commits derive notification audiences from resolved entity keys and old/new
compiled authorization. Newly authorized and revoked viewers both receive baselines;
unrelated viewers skip metadata workers before authentication or history I/O.
Bot credential/registration changes refresh the bot and owners, with compact command
catalogue invalidations for other current readers. Coalescing unions account and
catalogue audiences, including catalogue invalidations alongside forced recovery.
Directory identity changes target authorized directory viewers. Profile-only metadata commits preserve authority templates and patch bounded
cached avatar projections. Self-service avatar APIs use compact profile events.
Identity, membership and policy changes invalidate authority templates. The global policy revision is a version of
captured authority, not a promise that unrelated clients receive every server mutation.

SQLite uses schema v20. Borrowed persistence
projections serialize metadata without cloning policy/bot maps or constructing
empty-history domain objects. Owned pending messages supply browser and bot mutations. Production and test
command adapters use the same prepare/commit/publication operations; there is no
working-message scratch state or duplicate message/history mutation executor. Message actions resolve stable IDs through the existing conversation
index; reactions carry one membership delta and edits capture only prior attachment
IDs. Postcommit metadata invalidation is shared, while the outer command
receipt transaction still encloses nested savepoints and reloads on failure.

Cold snapshot/read-notification unread misses share one reader transaction and
reuse cached statements; per-conversation failures still omit only that entry.
Pagination uses shared ascending/descending SQL definitions, one edge-existence
query and allocation-free exact serialized-size counting. Revision-keyed caches,
reader limits, engine-lock release and transport authorization rechecks remain
part of the read contract.

Quoted replies use the timeline scroll animation for messages in loaded history,
including virtual rows. Older replies fetch their history page and jump directly;
this keeps navigation bounded without speculative requests.
