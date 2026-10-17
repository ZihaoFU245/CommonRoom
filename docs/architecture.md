# Architecture

CommonRoom has one Rust API/WebSocket executable, a separately built static UI served by Nginx, and one portable data folder. The web is a Preact application written in strict TypeScript. Node and pnpm are development/build dependencies, not production services.

## Server ownership

`server/src/main.rs` wires process lifecycle, configuration, shared application state, and graceful shutdown. `console.rs` owns the stdin superuser interface. `config.rs` loads and validates saved settings; settings require a restart.

`engine/mod.rs` defines the engine and module boundary. Domain modules extend that owner rather than creating separate copies of application state:

| Module | Responsibility |
| --- | --- |
| `models.rs` | Persisted data and serialized snapshot/history types |
| `storage.rs` | SQLite opening, transactional initialization, writer connection, bounded maintenance, cursors, checkpointing |
| `history.rs` | Four read-only connections, keyset pagination, bounded page/unread caches and cache statistics |
| `authorization.rs` and `authorization/` | Policy types, predefined bundles and compiled index; `access.rs` owns access checks/delegation, `commands.rs` owns policy inspection/mutations |
| `accounts.rs` | Account/session lifecycle, roles, provisioning, and account deletion |
| `rooms.rs` | Room creation, deletion, invitations, joining, and leaving |
| `messages.rs` | Message construction, private queues, reactions, replies, and cleanup |
| `queries.rs` | Authorization-filtered snapshots, history, directories, and unread calculations |
| `dispatch.rs` | Input validation, command routing, and rollback on rejected/failed mutations |
| `helpers.rs` | Password hashing, names, timestamps, mention scanning, and private-pair keys |
| `tests.rs` | Engine regression tests using isolated storage |

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
`engine/messages.rs`, transaction completion/rollback in `engine/dispatch.rs`,
and cursor persistence in `engine/queries.rs`. The bot adapter owns v1-specific
validation, retry receipts and error projection. SQL remains in the engine's
storage/history modules.
HTTP writes and compact notification/discovery sockets are separate from browser
commands/snapshots. Workers configure their own settings through advertised chat
commands and enforce their own owner/admin policy. Registrations and receipts use
stable ordinary account IDs. `web/scripts/bot/` is a reference worker with its own
checkpoint/outbox, not a server-side agent runtime.


`commands.rs` outside the engine owns help/completion metadata and the shared account-provisioning syntax used by stdin and web password jobs. `manual.rs` provides read-only command and grant-system manuals. The domain dispatcher executes commands; the registry does not authorize them.

`web/mod.rs` owns route construction and `App`. Its child modules separate proxy/origin security, authentication, history/read HTTP handlers, and WebSocket transport. `web/commands.rs` handles asynchronous account commands and configuration output. Password hashes are rechecked when committing after a background job. All SQLite access belongs in the engine storage/history modules.

One `Arc<RwLock<Engine>>` permits simultaneous engine readers and serializes mutations. Database/lock work runs on blocking workers; HTTP work admission is capped at 64 jobs. Never hold an engine guard across `.await`. HTTP history captures authorized resource identity/revision, releases the engine guard for SQLite I/O, then rechecks session, access and identity before returning a coherent revisioned page. SQLite WAL has one writer and four independent read-only connections. The standard OS RwLock does not promise a particular scheduling/fairness policy. Changes are broadcast after successful persistence; read-only commands do not rewrite state. Account deletion commits message/session changes and cursor cleanup together. In-memory rollback/cursor updates must retain this atomic behavior.

## Web ownership

`web/src/main.tsx` boots the app and selects login or chat. `components/Chat.tsx` composes the screen and connects feature hooks; individual components render navigation, messages, composer, local output, login, and settings.

- `api/protocol.ts` declares wire types and validates untrusted JSON. `api/client.ts` owns relative URLs, cookie-based requests, response validation, and outbound frames.
- `hooks/useConnection.ts` owns WebSocket lifetime, reconnect backoff, current snapshot, and HTTP resynchronization. Callback refs avoid stale state without reopening sockets on every render.
- `hooks/useHistory.ts` loads 100-message pages and keeps sliding conversation windows, rejecting stale responses.
- `features/conversation/historyCache.ts` bounds each window to 1,000 messages/approximately 4 MiB and the shared browser history cache to 5,000 messages/16 MiB/32 views. Oldest views are evicted.
- `hooks/useVirtualTimeline.ts` mounts the viewport plus eight rows of overscan, measures variable row heights and preserves a message anchor when pages move.
- `hooks/useReadTracking.ts` owns visible-message acknowledgements, scroll placement, and jump-to-unread.
- `hooks/useCommands.ts` coordinates aliases, reactions/retractions, output and navigation. `hooks/useCommandComposer.ts` owns drafts, completion, keyboard controls and reply selection. `features/conversation/requestLifecycle.ts` owns the shared single-request lifecycle and completion on send failure/disconnect; `submission.ts` captures confirmed command targets and constructs frames.
- `hooks/useLocalConsole.ts` owns the tab-local command ledger and clearing. It never persists command output.
- `hooks/useFonts.ts` and `useComposerResize.ts` own presentation preferences and input sizing.
- `hooks/useMessageEditorLayout.ts` owns inline editor sizing, focus, animation, and last-message scroll anchoring; `ConsoleMessage.tsx` owns rendering and edit interactions.
- `hooks/useTimelineMotion.ts` animates live message arrivals, retractions and edits with transform/opacity effects on mounted rows. It preserves both row and text positions when grouping or date headers change, fading incoming/outgoing headers. It waits for synchronous virtual measurements before painting, shares the editor's 180 ms ease-out timing, and keeps temporary inert exit copies outside virtual sizing and read tracking. Short histories temporarily extend the paint clip into unused viewport space without adding scroll range. Conversation identity, history readiness and revision changes prevent animations during initial loading, paging and view switches. Animation frames stop when the transition finishes; scrolling updates cached positions without geometry reads.
- `useHistory` distinguishes background revision refreshes from initial loading and explicit paging; refreshing an existing window does not insert a loading bar or move the conversation viewport.
  A jump target is cleared after its page commits, so later refreshes follow the
  visible anchor and live arrivals can resume merging into the loaded window.
- `features/conversation/` contains pure ordering, date, mention, and unread helpers. `features/preferences/` contains browser preference and notification helpers.
- `features/conversation/math.ts` renders untrusted LaTeX with bounded KaTeX settings. `components/Math.tsx` rebuilds that generated markup as Preact nodes and CSSOM styles under the chat CSP; message-supplied HTML never uses this trusted path. The Markdown token converter creates math nodes independently of HTML; generated nodes survive safe HTML filtering. Preferences shares the `/markdown` sending state and stores the separate LaTeX display preference locally.
- Markdown sending preferences/manuals live in the dependency-free `features/preferences/markdown.ts`. The message renderer uses a shared async loader without suspending virtual rows: plain-text chats do not load Markdown-it. Basic Markdown images use validated URLs and passive nodes. TeX tokenization and KaTeX are separate optional chunks; Security options and math default off.
- Security has two independent browser-local options. Allow HTML (secure HTML) automatically rebuilds sanitized passive HTML in the main DOM, without frames. HTML (code) offers a per-message Show text / Render HTML gate. Messages with CSS, scripts, active controls or other active elements use `MarkdownFrame.tsx` only after selection. When only HTML (code) is enabled, passive HTML also requires selection but renders directly through the sanitizer. A lightweight token detector skips optional HTML engines for code examples and autolinks. Full HTML documents bypass Markdown indentation parsing; fenced/code-span examples remain literal.
- `HtmlMarkdownGate.tsx` loads the optional classifier/sanitizer on demand and retains text on failures. `richMarkdown.ts` uses Markdown tokens and parse5 to classify raw HTML, strips active subtrees and attributes for the direct path, and preserves generated Markdown image/math nodes with source-excluded markers. `MessageList` bounds explicit choices to the current history window, resets edits/settings/conversation changes and memoizes state across scrolling. ResizeObserver keeps virtual heights current.
- `markdown-frame.html` is a single generated static resource with an inline bootstrap compiled from `markdownFrame.ts`. Vite exposes its content-versioned URL, and Nginx serves it with a dedicated CSP and immutable caching. It has no bundle/font/CSS subrequests. Each selected preview is an iframe with only `sandbox="allow-scripts"`; same-origin access, forms, popups and top navigation remain disabled. A one-shot parent message supplies the source; the frame applies a leading CSP again before executing the HTML. It reports bounded heights via source-checked messages and ResizeObserver, without polling. Frames unmount outside the virtual window; remounts use the browser's cached shell and restart the selected preview. No hidden frame pool or persistent game state is kept.
- The frame allows inline CSS/JavaScript and data images/media/fonts, but blocks fetches, external resources, nested frames, base URLs and form actions. The app's leading `frame-src 'self'` meta policy also applies without Nginx; the frame carries its own leading CSP meta. Production headers add frame-ancestor restrictions and preserve a strict script/style policy on the main app. Full HTML code remains untrusted and never runs with the chat's origin.


Server snapshots are authoritative for membership, scoped permissions and commands, peer directories, and messages. The legacy wire `admin` flag is informational; UI controls use effective grants. Browser storage contains presentation/notification preferences and browser-local command aliases. Stable event callbacks and memoized message lists keep draft edits from sorting and rendering the transcript again. Keep pure helpers independent of browser APIs where possible.

## Scaling and extension

Snapshots log and omit individual unavailable history tails or unread entries
while retaining healthy conversation data. Missing unread data is not reported
as zero unread. Poisoned history-store locks fail closed with a restart diagnostic;
HTTP snapshots return 503 and browser sockets send an error frame before closing.

This layout scales feature development by putting each change in its owning module. Preserve the single state owner and add modules when a responsibility becomes distinct. Extra crates, generic repositories, or a global frontend state framework are not required to add ordinary features.

`max_messages` is a disk retention limit per room/private pair (default 1,000), independent of cache capacity. Raising retention does not raise cache budgets. Startup loads entities, policy, sessions and read cursors, not retained message bodies. The current schema is v12; validated v9/v10/v11 databases upgrade transactionally, and older schemas are rejected.

The server creates cache pages lazily: at most 1,000 cached message entries per conversation, 10,000 globally, 256 pages, and 32 MiB of conservative payload accounting. These are cache budgets, not a promise about total process RSS; response copies, SQLite caches, metadata and temporary maintenance work also use memory. Unread results have a separate bounded 4,096-entry revision/cursor cache. `/configs` exposes page-cache hits, misses and usage to authorized operators.

Schema v7 adds indexes for stable message IDs, authors, timestamps, global maintenance sequence and conversation IDs. Frequent mutations use a small transient working set and targeted transactions; administrative rollback clones metadata rather than history. Account rename/deletion uses a reverse name index and 100-row batches inside one transaction; rename revises only conversations with matching message/reaction references. Cleanup/retraction and retention pruning execute in SQLite, so evicting a cache page never deletes disk history.

New sends/replies/reactions broadcast a `conversation` update only to authorized readers of that conversation. New private directories and policy changes use a full snapshot. Presence changes send only the authorized online directory, and extra tabs do not broadcast unchanged presence. Snapshot tails are globally capped at 1,000 messages/4 MiB serialized payload and at most 50 per conversation; the client independently bounds tail memory. Lagged subscribers receive a baseline snapshot. Full snapshots and uncached unread calculations still have costs proportional to authorized conversations/retained sequence ranges.

History uses stable sequence cursors, not deep OFFSET pagination. Requesting a range near sequence 1,000 reads that page directly without reading the first 999 messages. `around` probes at most half a page backwards before reading the page. Browsing beyond a 1,000-entry window evicts the opposite end; returning to it reloads only that page. The old all-history HTTP/UI path and all-history startup loader are removed. See [Data](data.md) and [Protocol](protocol.md).

This release is a single-process service. Multiple application replicas must not share a data folder; an OS lock enforces one owner. Horizontal scaling would require shared persistence and cross-process event delivery. A reverse proxy/load balancer currently supplies TLS and routes to that single process.

Socket baselines share serialized buffers per account and browser/bot profile.
The cache is bounded to 128 entries/32 MiB and keyed by mutation revision,
visible presence revision, and the account's read revision. Browser HTTP/socket
baselines share buffers; bot v1 HTTP/socket keys also include the credential ID.
Read-only commands and unrelated read cursors preserve baselines. Every caller
rechecks its credential before reuse. Concurrent readers share one baseline build per key; partial baselines are not retained. History responses
share immutable page slices and exact per-message wire sizes, so cache hits and
snapshot byte budgets do not copy or serialize message bodies again. Unread
lookup uses a hash index with bounded ordered recency tracking. Command lists
filter a shared catalogue using one effective-rights calculation per scope.
Browser cache entries retain byte counts and global running totals; ordered page
merges and directional trimming are linear. Ledger ingestion reuses its ID set
and skips sorting when no new message IDs arrive.

`engine/storage/command_receipts.rs` owns atomic native command outcomes. An outer
SQLite savepoint encloses ordinary command storage savepoints and its receipt;
receipt failure rolls back SQLite and reloads authoritative metadata/cursors
before broadcasting; successful commands do not clone all metadata/cursors.

Browser API failures retain HTTP status; 401 responses return the user to login.
Requests have a 15-second client deadline. Login sessions are capped at eight per
account with oldest-expiry eviction; the instance ceiling scales with max_users.
History retries preserve range/direction and stop after three attempts. A coherent
received page remains usable during live revision churn, with a refresh status.
Live snapshot tails enter the ledger even when paging fails. Read acknowledgements
use one pending timer reading current state, so incoming messages cannot starve it.

Schema v12 stores audit entries separately from policy JSON. Metadata saves write
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
