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

`server/src/bots/` owns the frozen machine profile, credential management and
runtime configuration. `bots/engine.rs` is an engine extension for typed chat
operations using stable conversation IDs. Browser and bot adapters share message
insertion, reply quotes, action authorization and reaction mutation in
`engine/messages.rs`, transaction completion/rollback in `engine/dispatch.rs`,
and cursor persistence in `engine/queries.rs`. The bot adapter owns v1-specific
validation, retry receipts and error projection. SQL remains in the engine's
storage/history modules.
HTTP writes and compact notification/config sockets are separate from browser
commands/snapshots. Runtime configuration uses a transient bounded request/reply
registry; the external worker owns its settings. Registrations and receipts use
stable ordinary account IDs. `web/scripts/bot/` is a reference worker with its own
checkpoint/outbox, not a server-side agent runtime.


`commands.rs` outside the engine owns help/completion metadata and the shared account-provisioning syntax used by stdin and web password jobs. `manual.rs` provides read-only command and grant-system manuals. The domain dispatcher executes commands; the registry does not authorize them.

`web/mod.rs` owns route construction and `App`. Its child modules separate proxy/origin security, authentication, history/read HTTP handlers, and WebSocket transport. `web/commands.rs` handles asynchronous account commands and configuration output. Password hashes are rechecked when committing after a background job. All SQLite access belongs in the engine storage/history modules.

One `Arc<RwLock<Engine>>` permits simultaneous engine readers and serializes mutations. Database/lock work runs on blocking workers; HTTP work admission is capped at 64 jobs. Never hold an engine guard across `.await`. HTTP history captures authorized resource identity/revision, releases the engine guard for SQLite I/O, then rechecks session, access, identity and revision before returning. SQLite WAL has one writer and four independent read-only connections. The standard OS RwLock does not promise a particular scheduling/fairness policy. Changes are broadcast after successful persistence; read-only commands do not rewrite state. Account deletion commits message/session changes and cursor cleanup together. In-memory rollback/cursor updates must retain this atomic behavior.

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
- `features/conversation/` contains pure ordering, date, mention, and unread helpers. `features/preferences/` contains browser preference and notification helpers.

Server snapshots are authoritative for membership, scoped permissions and commands, peer directories, and messages. The legacy wire `admin` flag is informational; UI controls use effective grants. Browser storage contains presentation/notification preferences and browser-local command aliases. Stable event callbacks and memoized message lists keep draft edits from sorting and rendering the transcript again. Keep pure helpers independent of browser APIs where possible.

## Scaling and extension

This layout scales feature development by putting each change in its owning module. Preserve the single state owner and add modules when a responsibility becomes distinct. Extra crates, generic repositories, or a global frontend state framework are not required to add ordinary features.

`max_messages` is a disk retention limit per room/private pair (default 1,000), independent of cache capacity. Raising retention does not raise cache budgets. Startup loads entities, policy, sessions and read cursors, not retained message bodies. The current schema is v9; validated v8 folders upgrade transactionally.

The server creates cache pages lazily: at most 1,000 cached message entries per conversation, 10,000 globally, 256 pages, and 32 MiB of conservative payload accounting. These are cache budgets, not a promise about total process RSS; response copies, SQLite caches, metadata and temporary maintenance work also use memory. Unread results have a separate bounded 4,096-entry revision/cursor cache. `/configs` exposes page-cache hits, misses and usage to authorized operators.

Schema v7 adds indexes for stable message IDs, authors, timestamps, global maintenance sequence and conversation IDs. Frequent mutations use a small transient working set and targeted transactions; administrative rollback clones metadata rather than history. Account rename/deletion streams 100-row batches inside one transaction. Cleanup/retraction and retention pruning execute in SQLite, so evicting a cache page never deletes disk history.

New sends/replies/reactions broadcast a `conversation` update only to authorized readers of that conversation. New private directories and policy/presence changes still use a full snapshot. Snapshot tails are globally capped at 1,000 messages/4 MiB serialized payload and at most 50 per conversation; the client independently bounds tail memory. Lagged subscribers receive a baseline snapshot. Full snapshots and uncached unread calculations still have costs proportional to authorized conversations/retained sequence ranges.

History uses stable sequence cursors, not deep OFFSET pagination. Requesting a range near sequence 1,000 reads that page directly without reading the first 999 messages. `around` probes at most half a page backwards before reading the page. Browsing beyond a 1,000-entry window evicts the opposite end; returning to it reloads only that page. The old all-history HTTP/UI path and all-history startup loader are removed. See [Data](data.md) and [Protocol](protocol.md).

This release is a single-process service. Multiple application replicas must not share a data folder; an OS lock enforces one owner. Horizontal scaling would require shared persistence and cross-process event delivery. A reverse proxy/load balancer currently supplies TLS and routes to that single process.
