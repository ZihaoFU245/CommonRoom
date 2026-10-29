# Development

## Setup

Use Rust 1.89+ (edition 2024), Node 22.18+ with native TypeScript stripping, pnpm, and RTK. The optional SimpleBot worker and its tests also require Deno 2.9.8+ with native KV enabled. Install the web dependencies from the committed lockfile:

```sh
rtk pnpm --dir web install --frozen-lockfile
rtk cargo run --locked
```

Create accounts in the server's stdin console:

```text
/user new alice alice admin
/user new bob bob user
/room new demo
/room add alice #demo
/room add bob #demo
```

Run the web development server in another terminal:

```sh
rtk pnpm --dir web dev
```

Open http://localhost:5173. Vite proxies `/api` and `/ws` to the Rust listener at `127.0.0.1:3000`. Development uses plain WS; release assets use HTTPS/WSS. The default Vite proxy assumes root hosting. To verify a configured base path, serve the built UI with Nginx or adjust the development proxy.

To build the debug server and UI:

```sh
rtk proxy ./auto/build.sh debug
rtk proxy ./auto/debug.sh
```

`debug.sh` runs the existing Rust debug binary and previews `web/dist-debug/`
at http://127.0.0.1:5173, proxying the API/WebSocket to `127.0.0.1:3000`.
It defaults to SQLite with no S3 operations. Debug data and its generated
`config.json` live in the repository's `data/` directory.

```sh
rtk proxy ./auto/debug.sh
rtk proxy ./auto/debug.sh --with-s3
rtk proxy ./auto/debug.sh --with-postgres
rtk proxy ./auto/debug.sh --with-s3 --with-postgres
```

Start dependencies in separate terminals before enabling them:

```sh
rtk proxy ./auto/start-s3.sh
rtk proxy ./auto/start-postgres.sh
```

`--with-s3` connects to an already-running local S3 service and prepares the
bucket/CORS settings. The optional RustFS helper requires `tmp/rustfs`; another
local S3 service can be started separately. `--with-postgres` connects to an
already-running database, defaulting to `postgresql://cr:pass@127.0.0.1:5432/devdb`;
`CHAT_DATABASE_URL` can override it. Debug never starts or stops either dependency.
Credentials remain environment-only. The Rust debug build defaults to development
mode; release builds are always production. Node is only used for the existing UI
preview. Ctrl+C or a service exiting stops chat and UI. It does not rebuild. Release
packaging uses `rtk proxy ./auto/build.sh release` and produces `./chat` and
`./ui.tar.xz` (containing `dist/`).

All shell scripts live in `auto/` and locate the repository root themselves.
Run `rtk proxy ./auto/clean.sh` to remove `target/`, both UI bundles, Vite
caches, `./chat`, `./ui.tar.xz`, and RustFS runtime files in `tmp/rustfs-local/`
(including local buckets). Stop running debug and RustFS processes first.
Cleaning preserves `tmp/rustfs`, other `tmp/` files, `data/`, installed
dependencies, lockfiles, and the PostgreSQL cluster in `pgdata/`.
Both build modes skip source checks and tests. Run `rtk proxy ./auto/check.sh`
separately before deploying.

Set `CHAT_DATA` to an isolated directory for experiments. Existing `data/` contains real account credentials, sessions, and private history; do not delete or modify it for tests. Tests create their own temporary folders. Stop a server before copying its entire data folder.

SQLite is the default. For PostgreSQL, select `database.backend=postgres` in
`config.json` and set `CHAT_DATABASE_URL` to an isolated database; see
[PostgreSQL](postgresql.md) for connection settings and the
built-in SQLite transfer. History, snapshots and browser/bot notification plans
await the typed SQLx storage interface. Use `App::inspect_engine` for owned
metadata capture/revalidation; it performs only memory checks while holding the
engine guard. Await database health and reads outside that guard, then revalidate
authority before delivery. Browser sends and `/reply`, `/edit`, `/react`, `/retract`,
`/clean` actions,
bot sends/reactions and read acknowledgements use conversation permits plus
`App::with_conversation` for short owned preparation/publication. They await storage
outside the engine guard while sharing a metadata fence. Administrative plans
acquire that fence exclusively and await native writes outside the engine guard.
The old synchronous command executors and storage write adapters compile only in
tests. Bootstrap/offline transfer and final pool draining retain the persistent
storage runtime; live queries and mutations await SQLx directly. Bulk history cleanup
counts outside conversation permits, then acquires matching conversation locks
before the shared metadata fence. Recheck every target revision and the full target
set before committing; changed plans retry at most three times. Keep deletion and
audit atomic. Audited deletions additionally hold the audit permit through
commit/publication; sends and cursors do not need that permit. No runtime belongs to a
connection, and pools drain before ownership locks are released.

Memory capture/publication first tries an uncontended engine guard on the async
task, falling back to blocking dispatch on contention. Never await with a guard.
Known durable publication failures schedule one full authoritative metadata/cursor
reload under the exclusive metadata fence. Validate and compile replacement state
before installing it, invalidate caches and broadcast resynchronization afterward.
Failed recovery stays unavailable; unknown commits, ownership loss and poisoned
locks must not enter this recovery path. Shutdown must drain queued recovery.

Read-only browser commands use owned plans. `/history`, elevated cleanup previews
and `/system` await native readers/statistics outside engine guards; memory-only
commands use the same capture/revalidation path. Sudo query phases elevate only
inside their synchronous guard and recheck authority before delivery. Avatar/color
updates use per-account ordering and atomic profile/media transactions. Accepted
attachment/avatar completions own the object mutex through commit/publication;
transport cancellation must not make a pending object available for finalization
again while its first database commit is still running.

Basic administrative mutations use `App::with_metadata` and an asynchronously
acquired exclusive metadata permit. The permit remains held through native commit
and publication, while the engine guard covers only preparation/publication.
`PendingMetadata` owns resolved target snapshots and serialized typed operations;
preparation restores the original memory state before returning. Only committed
deltas become visible. Publication must run even if final response authorization
fails after a session expires during database I/O.

Account metadata and room rename share pure plans with compatibility callers.
Credential commands hash before acquiring the native metadata permit and recheck
authority/identity afterward. Login/logout use `SessionWrite` for atomic session
and event persistence; accepted login work retains its attempt/password permits
through cancellation. Browser socket/session authentication checks native storage
health outside engine guards, then verifies the stable principal again. Presence
updates must remain memory-only.

Versioned bot credential/conversation mutations use owned metadata deltas and
retain accepted work through commit/publication. Bot source facts and receipt
lookup use reader leases, then recheck live credentials and target authority before
delivery. Socket authentication, activity and command discovery await health outside
engine guards; their final metadata/runtime checks are memory-only. Bot commands prepare owned effects and commit effect/outcome receipts in one
native transaction. Initial preparation uses a shared metadata permit; metadata
plans upgrade to an exclusive permit and message/history plans reacquire sorted
conversation permits, then reprepare and verify their targets. Only audited
history deletions acquire the audit permit. PostgreSQL locks the submitting
account before receipt replay/capacity checks and before any append counter or
conversation row. Replays publish no effect, rejected domain outcomes persist
without effects, and unavailable failures persist no terminal receipt.

Elevated message and cleanup commands use the same native operation paths as
ordinary browser writes. Each synchronous capture/preparation stage revalidates
session identity and sudo grants, installs authority only within that phase, then
restores compiled authorization before awaiting storage. Conversation permits
retain ordering through commit/publication; publication of committed work must
finish even after transport cancellation. Initial message lookup without a known
conversation holds no metadata permit, so later preparation must recheck sudo.

Console input is read on a blocking thread that holds no application/database
reference. A bounded channel dispatches accepted commands to native async plans;
credential secrets return only to stdout. Trusted stdin edits/retractions and
cleanup use the same message/history operations with the console principal,
without creating a synthetic account. Console direct sends/replies/reactions still
require a user account. Startup retention/media cleanup and shutdown checkpointing
also await native storage. Shutdown stops new admission, drains both general I/O
and password-only jobs through publication, then checkpoints without an engine
guard. The synchronous engine lock helpers are test-only.

## Browser compatibility

The minimum supported Chrome version is 114. Production JavaScript and CSS targets
pin `chrome114` in Vite while preserving the other existing browser targets.
Build targets handle syntax, not missing runtime APIs. Feature-detect newer APIs
and keep fallbacks in focused helpers; do not infer support from the user agent.

All request cancellation/deadlines go through `api/requestAbort.ts`. It uses native
`AbortSignal.any` when available and `AbortController` forwarding on Chrome 114.
It owns timeout timers for consistent `TimeoutError` reasons and releases timers
and listeners in `finally`, after response bodies finish. ESLint rejects direct
`AbortSignal.any`/`timeout` calls elsewhere. The browser floor applies to the UI;
Node/Deno development and bot-worker requirements remain separate.

## Coding conventions

- Read [Architecture](architecture.md) before changing state ownership or module boundaries.
- Prefix commands you execute with RTK. Use `rtk proxy` for unsupported commands. Build/run scripts themselves contain ordinary commands; do not insert RTK into their bodies.
- Write application web code in `.ts`/`.tsx`. Keep strict null/index/optional-property checks, explicit return paths, switch fallthrough checks, and unused/unreachable-code checks enabled. Use `unknown` at JSON boundaries, validate it, then narrow to a protocol type. Avoid `any`, non-null assertions, unchecked protocol casts, `@ts-ignore`, and disabling checks to make a build pass.
- `pnpm --dir web typecheck` explicitly uses TypeScript 7 via the `@typescript/native` alias. The separate `typescript` 6 dependency supplies the supported compiler API for type-aware ESLint. Keep both lockfile versions compatible with their tooling; the default bare `tsc` executable belongs to the API dependency. ESLint rejects unsafe assignments/returns, unhandled promises, asynchronous callbacks used as void handlers, and non-null assertions. Handle promises with `await` or an explicit rejection handler; `void promise` does not bypass the rule.
- Declare component props and type DOM refs/events. Keep rendering components separate from connection, history, and command state. Prefer pure feature helpers and focused hooks.
- Preserve current UI conventions: the light theme keeps its paper background and dark
  normal text, both themes keep orange primary buttons and unread badges, and chat stays
  left-aligned with browser-local dates and Unicode message text. Themed surfaces read
  the palette custom properties at the top of `style.css`; add a new color to `:root`
  and to `:root[data-theme="dark"]` rather than hardcoding it in a rule, then add the
  same token to `paletteTokens` and derive it in `features/preferences/palette.ts`. A
  custom palette writes every entry of that list as an inline property, so a token
  missing from it silently keeps the built-in value. Text drawn on `--accent` must use
  `--accent-label` or `--accent-contrast`, never `--ink`, because the accent fill does not
  change between themes.
- Small controls, settings reveals and menus use `controlMotion` from
  `web/src/features/preferences/motion.ts` (120 ms, ease-out). `applyAppearance`
  writes the matching CSS motion variables before rendering; the stylesheet keeps
  first-paint fallbacks. Message and preview motion retain their separate 180 ms
  timing. Respect reduced motion and clean up animation controllers/listeners.
- Keep business permissions in the engine. Browser visibility/completion is not authorization. Keep all SQL, schemas, connection/session and COPY mechanics in the independent SQLite/PostgreSQL modules under `engine/database/`. Shared `engine/storage/` coordinates typed operations and committed publication; `engine/history/` owns shared caches. Avoid holding the engine read/write lock across `.await`.
- Cargo manifests forbid application unsafe code and ignored must-use values. Clippy rejects narrowing/sign-changing casts, `unwrap`/`expect`, explicit panic/unreachable/todo/debug macros. Return errors for invalid state and conversions. `unwrap`/`expect` are allowed only in test modules; deliberately injected panics need a local documented test allowance. Never recover a poisoned engine with `into_inner`; return unavailable and require a restart.
- Preserve password masking, session revocation, per-conversation retention, monotonically advancing read cursors, and tab-local command output. Never log credentials or message bodies.
- Preserve compatibility with existing HEAD-schema `data/`. Published schema changes need a version increment, explicit compatibility/rejection tests, rollback behavior, and an update to [Data](data.md). SQLite startup supports fresh/HEAD v20 and an atomic v19-to-v20 credential upgrade. PostgreSQL is unused and unpublished: modify its independent current v1 schema directly, without historical WIP upgrades. Unsupported schema versions are rejected unchanged. Always maintain a read-only HEAD SQLite → current PostgreSQL transfer; PostgreSQL changes must not force a SQLite migration.
- Use `cargo fmt` and the web Prettier scripts. Update the relevant guides when paths, commands, protocol, configuration, or deployment behavior changes.

## Adding a feature

1. Put its persisted/wire model in `engine/models.rs` when needed.
2. Implement domain behavior in its owning engine module; route commands in `dispatch.rs` and register help/completion metadata in `server/src/commands.rs`.
3. Update `web/src/api/protocol.ts` types and runtime validators together with the Rust wire contract. Add/update contract checks in the integration harness.
4. Implement presentation in a component and stateful behavior in a focused hook; reuse the pure feature helpers.
5. Add tests for permissions, persistence failures, privacy, or synchronization risks introduced by the change. Run the checks in [Testing](testing.md).

The JavaScript files in `web/tests/` and `web/scripts/` are runtime test harnesses. Node loads the application's TypeScript helpers directly; fixtures intentionally include malformed/partial input. Production application source and Vite configuration are TypeScript and are checked before every web build.

## External agent development

Start with [Agent development](agent-development.md). The reference worker is
`web/scripts/bot/agent.mjs`; its client and tests use the existing Node/`ws`
development dependencies. It is independent of the browser UI. Extend its handler
with model/tool calls while preserving its outbox and cursor ordering. Keep the published v1 schema, worker and contract tests aligned.
Keep `/api/bot/v1/` as the sole bot API namespace; no alternative version is planned.
Browser identity resolution must remain session-only. Expose machine capabilities
through explicit v1 adapters and shared services, never by enabling bearer
authentication in browser handlers.

The debug WebSocket proxy disconnects paired streams when either peer ends its
TCP stream, avoiding late writes during navigation/logout. Transport failures
still use the normal Vite error reporting.

SimpleBot's optional shell capability is enabled only by
`deno task start --with-shell` (or `deno task dev --with-shell`). Its launcher
grants Deno binary execution before worker startup; direct invocations must supply the matching runtime
permissions. The tool starts in the workspace and leaves filesystem, environment
and network policy to the deployment. See [SimpleBot](../simpleBot/README.md#optional-shell-and-binary-execution).

`auto/debug.sh --with-s3` uses local RustFS at `127.0.0.1:19000`, bucket
`commonroom-draft`, and `cr` / `pass` development credentials. `RUSTFS_ACCESS_KEY`,
`RUSTFS_SECRET_KEY` and `CHAT_S3_BUCKET` can override those defaults. Start the
service separately with `auto/start-s3.sh` or another local S3 launcher; do not
run a second instance on the same ports.
