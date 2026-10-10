# Architecture

CommonRoom has one Rust API/WebSocket executable, a separately built static UI served by Nginx, and one portable data folder. The web is a Preact application written in strict TypeScript. Node and pnpm are development/build dependencies, not production services.

## Server ownership

`server/src/main.rs` wires process lifecycle, configuration, shared application state, and graceful shutdown. `console.rs` owns the stdin superuser interface. `config.rs` loads and validates saved settings; settings require a restart.

`engine/mod.rs` defines the engine and module boundary. Domain modules extend that owner rather than creating separate copies of application state:

| Module | Responsibility |
| --- | --- |
| `models.rs` | Persisted data and serialized snapshot/history types |
| `storage.rs` | SQLite opening, schema validation, locking, writes, cursor persistence, and checkpointing |
| `authorization.rs` | Action/command grants, predefined group bundles, scope/ownership policies, compiled permission index, and audit |
| `accounts.rs` | Account/session lifecycle, roles, provisioning, and account deletion |
| `agents.rs` | Agent accounts, provider keys, reply modes, renames, and reply triggers |
| `rooms.rs` | Room creation, deletion, invitations, joining, and leaving |
| `messages.rs` | Message construction, private queues, reactions, replies, agent answers, and cleanup |
| `queries.rs` | Authorization-filtered snapshots, history, directories, and unread calculations |
| `dispatch.rs` | Input validation, command routing, and rollback on rejected/failed mutations |
| `helpers.rs` | Password hashing, names, timestamps, mention scanning, and private-pair keys |
| `tests.rs` | Engine regression tests using isolated storage |

`commands.rs` outside the engine is the help/completion registry. `manual.rs` provides read-only command and grant-system manuals. The domain dispatcher executes commands; the registry does not authorize them.

`web/mod.rs` owns route construction and `App`. Its child modules separate proxy/origin security, authentication, history/read HTTP handlers, and WebSocket transport. `web/commands.rs` handles asynchronous account commands and configuration output; `web/agent.rs` is the only module that talks to the model provider, and `web/search.rs` the only one that talks to the search provider. Agent authorization lives in `engine/authorization.rs` beside every other grant, so the provider registry and the URL and model validation in `engine/agents.rs` never decide authority on their own. Password hashes are rechecked when committing after a background job. All SQLite access belongs in the engine storage module.

One `Arc<Mutex<Engine>>` serializes state changes. Never hold its lock across `.await`. Changes are broadcast after successful persistence; read-only commands do not rewrite state. Account deletion commits message/session changes and cursor cleanup together. In-memory rollback/cursor updates must retain this atomic behavior.

## Agent replies

An agent is an `Account` with `agent: true`, an owner, a provider key, a reply mode, optional personality text, and a provider selection: a provider name, a base URL, and a model id. Every supported provider speaks the OpenAI chat-completions shape, so one request builder serves all of them, and only the thinking controls differ per provider. The personality is appended to the base system prompt as a persona, so it cannot replace the rules that keep answers in the conversation's language and free of provider internals. Agents hold no password hash and no sessions, so they cannot log in, never appear online, and are refused every administrator permission.

`Engine::run` returns its console text together with the agent work the input triggered. The engine never performs network I/O: `messages.rs` queues one `AgentJob` per triggered agent, and `web/commands.rs` spawns the provider call after the engine lock is released, then re-acquires it to publish the answer and broadcast the change. `insert_agent_reply` snapshots the state before the answer consumes a sequence number, so a rejected write leaves no stored message and no consumed sequence. A per-agent pending set guarantees that one trigger produces exactly one answer, and an agent message never triggers another agent, so agents cannot answer each other endlessly.

Conversation context is the last `AGENT_CONTEXT` retained messages of the triggering conversation. A rename, a removal, an eviction, or a failed write must leave the engine, the retained history, and the provider view consistent.

With web search enabled, a `AgentJob` also carries the agent's search credential, and `web/agent.rs` runs three provider calls at most: one to decide whether the question needs current information, one search, and one answer prompt that contains the results. Sources travel back as a `来源：` link list appended to the answer. Every credential stays in the job; the snapshot carries only roles.

## Web ownership

`web/src/main.tsx` boots the app and selects login or chat. `components/Chat.tsx` composes the screen and connects feature hooks; individual components render navigation, messages, composer, local output, login, and settings.

- `api/protocol.ts` declares wire types and validates untrusted JSON. `api/client.ts` owns relative URLs, cookie-based requests, response validation, and outbound frames.
- `hooks/useConnection.ts` owns WebSocket lifetime, reconnect backoff, current snapshot, and HTTP resynchronization. Callback refs avoid stale state without reopening sockets on every render.
- `hooks/useHistory.ts` fetches and caches one selected conversation and rejects stale responses.
- `hooks/useReadTracking.ts` owns visible-message acknowledgements, scroll placement, and jump-to-unread.
- `hooks/useCommands.ts` owns drafts, suggestions, pending request IDs, reactions/replies, and command acknowledgements.
- `hooks/useLocalConsole.ts` owns the tab-local command ledger and clearing. It never persists command output.
- `hooks/useFonts.ts` and `useComposerResize.ts` own presentation preferences and input sizing.
- `features/conversation/` contains pure ordering, date, mention, and unread helpers. `features/preferences/` contains browser preference and notification helpers.

Server snapshots are authoritative for membership, scoped permissions and commands, peer directories, and messages. The legacy wire `admin` flag is informational; UI controls use effective grants. A snapshot also labels every visible account (`roles`, where `agent` comes from the account record and `admin` from the policy) and every room member that is an agent (`rooms[].agents`), so the sidebar, member directory, and message headers can mark agents without guessing. Browser storage contains presentation/notification preferences only. Stable event callbacks and memoized message lists keep draft edits from sorting and rendering the transcript again. Keep pure helpers independent of browser APIs where possible.

## Scaling and extension

This layout scales feature development by putting each change in its owning module. Preserve the single state owner and add modules when a responsibility becomes distinct. Extra crates, generic repositories, or a global frontend state framework are not required to add ordinary features.

Runtime capacity remains deliberately bounded by `max_users`, `max_rooms`, and `max_messages`; rooms and each private pair have independent deque histories. Append/oldest-message eviction is O(1). Snapshot tails contain up to 50 messages per conversation; the selected conversation fetches retained history on demand. Read cursor writes use a separate indexed table.

The current persistence format still clones and serializes the entire retained message/account state on mutations. A refactor alone does not make a complete send O(1). For substantially larger deployments, the next storage step is entity/message rows and targeted transactions, with a versioned migration and failure/rollback tests. The storage boundary now contains the SQL for that work. Benchmark full sends, snapshots, and memory before changing the storage format.

This release is a single-process service. Multiple application replicas must not share a data folder; an OS lock enforces one owner. Horizontal scaling would require shared persistence and cross-process event delivery. A reverse proxy/load balancer currently supplies TLS and routes to that single process.
