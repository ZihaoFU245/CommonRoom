# Data

SQLite is the default backend. Select `database.backend=postgres` in `config.json`
and provide its URL through `CHAT_DATABASE_URL` to use PostgreSQL;
see [PostgreSQL](postgresql.md) for startup, the verified offline SQLite transfer
command, and PostgreSQL backups. The file layout and copy-folder procedures below
describe SQLite deployments. PostgreSQL deployments still keep local configuration
and worker files, but chat accounts/history live in PostgreSQL.

## Database compatibility

SQLite HEAD remains **v19**, matching the published SQLite format. Empty SQLite
databases initialize as v19; existing v19 files open without a schema upgrade.
Other versions and unversioned existing databases are rejected before journal,
schema or data changes. PostgreSQL work must not force a SQLite version bump.

PostgreSQL is unpublished and uses its own **v1** schema marker. Fixes are folded
directly into this unused format. Empty targets initialize transactionally with
all current indexes and statement-level deletion retention. Existing targets must
match current v1; historical WIP versions have no upgrade path and unsupported
schema markers are rejected unchanged. SQLite remains v19 and transfers read-only
into current PostgreSQL v1.

The offline SQLite-to-PostgreSQL transfer is always maintained from **HEAD SQLite
to current PostgreSQL**. It opens SQLite read-only, checks its HEAD schema marker,
verifies all copied/derived table data, preserves source bytes, and commits the
new PostgreSQL schema and data together. It never upgrades or rewrites SQLite.
See [PostgreSQL](postgresql.md) for the command and backup procedures.

Startup reconstructs retention when the entire derived table is empty after a
data-only restore. PostgreSQL runs this offline rebuild without the runtime
statement deadline; the runtime deadline is restored afterward. Partially damaged
restores still require integrity checks and operator repair.

Session keys are domain-separated Blake2s digests. Root-path production sessions
use `__Host-chat_session` cookies. Color capabilities are direct room grants;
revoked grants stay revoked across restart and rename.

Authentication events retain the most recent 2,048 verified login outcomes and
recognized logouts (including expired stored sessions), including time, canonical client IP when available and valid
username. They contain no password, token or request body. Session creation and
logout commit atomically with their event. Throttled requests and malformed
credentials do not create authentication events.

Bot command receipts for `/system` reports include an optional authority marker
inside their existing JSON result. Replaying or retrieving these reports checks
the caller's current su or explicitly used sudo authority. This adds no tables or
schema version change; ordinary receipts retain their existing format.

Back up the entire data folder and object store with the server stopped. Tests
use isolated temporary data and never modify working `data/`.

## Move to another machine

1. Stop the old server and any reference agent using its data folder with Ctrl + C
   or SIGTERM. Wait for them to exit.
2. Copy the **entire `data/` folder** to the new machine, preserving it privately.
3. Place a compatible `chat` binary built for that machine's OS/architecture
   beside `data/`. Start it from that directory with `rtk proxy ./chat`.
4. Deploy the matching UI `dist/` folder to Nginx (extract `ui.tar.xz`) and
   configure static serving plus API/WebSocket proxying as described in
   [Deployment](deployment.md). Route the same public HTTPS hostname to the new
   machine and keep the proxy configuration equivalent. If the hostname changes, set `CHAT_ORIGIN` on
   the next start; browser cookies are tied to the original hostname.

```text
service/
├── chat                 # destination-platform backend binary
└── data/
    ├── config.json      # listener, public origin, production mode
    ├── chat.sqlite      # accounts, password hashes, rooms, memberships,
    │                    # messages, read positions, bot retry receipts, login sessions
    ├── chat.lock        # exclusive OS lock; safe to move after shutdown
    └── bot-agent-state.json # optional reference-agent checkpoint/outbox
```

SQLite may also have `chat.sqlite-wal` and `chat.sqlite-shm`; copy the whole
folder, never just the database while it is running. The stored format uses
portable SQLite and JSON, with no machine-specific paths or secrets outside
the data folder. Only one process can use a folder at once. This binary reads only the current v19 schema. Browser login sessions keep their original 12-hour expiry across
restart and moving the folder. Accounts, roles, memberships, message timestamps and
IDs remain the same. Proxy certificates, DNS, and OS-specific binaries are
deployment infrastructure and must be supplied on the destination.

On Unix, startup restricts the data directory to its owner (`0700`). This
folder contains private messages and active session credentials; keep
backups private as well.

Account renames use the reverse name index. Account state and renamed account/private read cursor keys commit in one transaction; failed writes preserve the original name and cursors.

Room renames retain the existing metadata layout. Room metadata and every name-based read cursor move
in one transaction. Stable room IDs keep history, ownership and scoped grants
attached to the renamed room. Failed writes preserve the old name, revision,
audit and cursors. Destination names do not inherit orphaned read positions.

## Storage layout and costs

Room deletion removes that room's read-position rows in the metadata transaction.
Cached cursors are cleared only after commit; failed writes preserve both room
state and read positions. Recreating the room name starts without old cursors.
This cleanup uses the existing tables and does not change the database schema.

`entities` stores one account, session, room metadata, or private-conversation
metadata per row. `messages` is keyed by stable conversation ID and sequence;
`reactions` stores individual reaction memberships and cascades when a message is
deleted. The small `state` row contains policy and bot metadata, and `counters` contains
the global sequence. Room/private revisions are separate columns, so a send does
not serialize memberships or policy. Composite tables use `WITHOUT ROWID` to avoid
a redundant row index; cursor cleanup also has a conversation index.

The current schema includes message-ID, author/sequence, time, global sequence and conversation-ID indexes.

Sends/replies insert one message, prune beyond the configured per-conversation retention, and update counter/revision atomically. A reaction changes membership rows and one revision. SQL range lookups use the conversation/sequence primary key. Cached pages are keyed by stable conversation identity, revision and range, so reaction/cleanup/user rename changes cannot reuse stale content.

Startup loads metadata and read positions only. Bodies and reactions are loaded in pages of at most 100. SQLite has one writer and two to four readers using WAL; concurrent history reads do not hold the engine writer lock. Cache eviction has no persistence effect. Server pages default to 1,000 entries per conversation, 10,000 total, and 32 MiB of conservative payload accounting. `database.history_cache` tunes the global message and byte budgets on either backend; SQLite page caches and response copies are additional memory. PostgreSQL sizes message/reaction payloads before fetching a bounded page prefix; a single oversized imported message remains allowed for pagination progress. See [PostgreSQL](postgresql.md#runtime-and-backups) for pool, cache and diagnostic settings.

Account rename/deletion reads only indexed references in batches of at most 100 message records inside the atomic transaction with session/cursor changes. Cleanup and retention pruning remain transactional and may process many disk rows, but no longer clone all history in memory. Administrative rollback retains only resolved entity keys, excluding transient message bodies. Creation and rename include destination keys so rollback also removes tentative inserts. Cleanup counting batches authorized IDs against
the existing timestamp expression index; this requires no schema change. The old v6 measurements in [Storage benchmarks](storage-benchmarks.md) are historical and do not establish current throughput.

Message JSON may include a boolean `markdown` field. Missing fields default to
false, preserving plain-text rendering for existing messages. This additive JSON
field does not change the SQLite schema or require a migration.

Message JSON may include an `edited` boolean. Missing values default to false.
This additive field uses the existing message storage. Edits update the
message payload and conversation revision in one transaction; failed writes
roll back both. Existing reply quotes retain the text and original sequence
captured when replying; quotes saved before the sequence field decode it as
zero (unknown) without a migration.

## Bot retry receipts

`bot_receipts` stores the stable bot account ID, caller key, request digest,
stable conversation ID, resulting sequence and original send time. It contains
no credential or copied message text. Inserts share the message transaction.
Receipts survive restart, account/room rename and credential rotation. Account
deletion clears its receipts atomically with the other account data. Removing a
message leaves its receipt intact until expiry so a retry cannot recreate it.

A receipt is usable for 24 hours. New keyed sends remove expired rows; each
account admits at most 4,096 live rows, rejecting capacity exhaustion rather than
evicting a still-valid key. These are disk records, not startup-loaded history.
See [Bots](bots.md#retries-and-history) for replay, access and expiry semantics.
The reference agent's checkpoint/outbox defaults to `data/bot-agent-state.json`
(under `CHAT_DATA` when set), alongside the server files. `BOT_STATE` overrides
its path. The agent owns this file independently; the server does not read or
modify it. A separately deployed agent keeps its own local data folder.

Bot credential JSON may include `expires`, `conversations` and `operations`.
Operator-issued provisioning credentials leave these null; v1-issued worker
credentials persist restricted scopes and an expiry. These metadata additions
use the existing entities JSON and require no SQLite schema migration.

Bot command catalogues are transient socket-owned metadata. They are not stored
in SQLite or bot checkpoints, disappear on disconnect/restart, and are queried
again from the worker. Command discovery itself requires no schema migration.

Bot registrations may include `owner_id` in their existing JSON metadata. Missing
values mean administrator control, so the ownership metadata needs no separate
SQLite migration. Stable IDs preserve ownership through renames; account deletion
clears owned bot references transactionally. Worker task checkpoints may include
private-command markers and native command intent/results; pending uncertain
effects are retained. Keyed pending commands are resolved/retried within the
receipt window; legacy unkeyed or expired effects require inspection.

## Command retry receipts

`bot_command_receipts` stores account ID, request key/digest, creation time and
serialized command outcome. The command's nested storage savepoints and this
receipt share one outer transaction. Failed receipt writes roll back command
metadata, messages, cursors and compiled in-memory authority. Receipts are not
loaded as startup history; lookups use the account/key primary key. Per-account
entry/byte limits and 24-hour expiry bound storage. See [Bots](bots.md) for retry
and authenticated status semantics. These outcomes may include private text.

## SimpleBot durable storage

The optional SimpleBot worker uses local Deno KV in `simpleBot/data/agent.kv`
(`BOT_STORAGE` overrides the path; `CHAT_DATA` moves the default directory),
independently of server SQLite schema v19 and the minimal reference worker.
Deno runs it with `--unstable-kv`. Old `checkpoint.json` and `BOT_STATE` are
ignored; no legacy checkpoint migration is performed.

KV format v2 stores account binding, input scan cursors, revisions, accepted
message versions, tasks, tick sessions/continuations, durable jobs, command
outcomes and the delivery outbox. Opening validated v1 worker data upgrades its
format marker atomically while retaining existing tasks, payloads and retry
keys; unsupported versions are rejected. This does not change server schema v19.
Saved v1 group decisions (including silence), batch membership and pending
outbox keys recover without fresh inference. KV v2 adds an optional per-task
`replyHash` marking the input version with a committed final response plan;
the marker is saved with the outbox and survives restart. Existing v2 data stays
readable, and existing matching output records seed that marker on open. Migration
is forward-only: the old v1 worker cannot open a v2 database. Back up while the
worker is stopped before upgrading if a downgrade may be needed.
Scan cursors advance on durable input admission; tick consumption and delivery
acknowledgements are separate. Interrupted model attempts become runnable again;
committed decisions are not regenerated. Interrupted jobs retain uncertain
terminal results instead of automatically replaying arbitrary effects. Changed
records commit asynchronously; large records stage immutable chunks before
atomic publication. Interrupted chunks older than a day are reclaimed at startup.
Pending keyed sends/commands retain their payloads and keys across restarts.
One worker owns each database. Stop it before backing up the data directory,
including SQLite sidecars. No server schema migration is required.

The same database holds model-owned JSON documents, scoped by bot account and
stable conversation ID. Rooms share a workspace across participants; DMs and
other rooms remain isolated. Workspace documents survive task pruning and
history retention and require explicit model/operator deletion. Source edits or
retractions do not automatically remove derived documents. Operator configuration
remains in `config.json`, outside model access. See [SimpleBot](../simpleBot/README.md#model-owned-persistent-workspace).

SimpleBot's `data/workspace/` is the model's persistent filesystem. It is shared
across all this worker's rooms and DMs; conversation-scoped KV memory remains
separate. `CHAT_DATA` moves the entire default data directory. Subprocess runtime
caches use temporary `.execution-*` directories beside the workspace and are
removed after each run; an abrupt worker kill may leave a cache directory that
an operator can remove while the worker is stopped. Scripts and artifacts are
ordinary files, outside the worker KV format and server SQLite schema. The model
may modify or delete any file in its workspace. Back up that directory with the
worker's other data when preserving scripts or generated artifacts matters.

Optional shell jobs use the existing v2 durable job records, including command
arguments/results and the optional `waitExpired` boolean. No server or worker
format migration is required. The `--with-shell` capability is process state,
never configuration or a persisted grant. Interrupted commands are not replayed
when reopening storage; launching without shell access does not restore that
capability from old jobs.

Metadata commands persist only changed entities and removed identities. Retraction
and cleanup delete affected message rows, update affected conversation revisions,
and append their audit record in one savepoint. Explicit cleanup keeps reply quote
text, as before; retraction scrubs it through the reply-target index. Native bot
command receipts still enclose the complete effect; rollback reloads authoritative
metadata/cursors only after failure rather than pre-cloning them on every request.
Expired sessions are filtered at startup and removed from disk during login.

## Media objects

`media` stores pending/ready object metadata and stable account/conversation IDs.
`message_media` references `(conversation,sequence)` and cascades on message
deletion. References are reconciled in each message transaction, including edits;
failed writes roll back both. Media metadata stays in SQLite, bytes in a private
S3-compatible bucket. Back up and restore **both** together when uploads are enabled.
See [Media uploads](media-uploads.md) for lifecycle, cleanup and deployment.

Room media-upload restrictions are stored by stable account/room IDs in policy
JSON's defaulted `media_revocations` map. Existing room user/admin assignments gain
default uploads at policy compilation. Restrictions commit with audit/revision
updates, survive renames and owner transfers, and are removed on account/room
deletion. This additive JSON field requires no SQL schema change.

Media cleanup uses partial/covering indexes, retry timestamps and a reference
removal trigger. SQL ready/expiry/deletion columns are authoritative when loading.

## Account metadata

Each account entity stores `metadata`, a map from `@global` or stable room ID to
a bounded string map. This is a two-level override tree, not a recursively nested
room hierarchy: `@global` is the root, stable room IDs identify sparse branches.
Only explicit values are persisted; reads resolve room → global → configuration
without copying inherited/default values into a branch. Account-wide quota is
root-only. No schema change is needed for this read projection.
Rename keeps it on the same stable identity; room deletion
removes that room's entries from every account in the same transaction as room
deletion. Account deletion removes the complete map. Empty branches are removed
on unset; reusing a room name creates a new ID and inherits no deleted overrides.
Metadata updates save changed accounts and audit entries atomically. Other values are
never included in public snapshots; the global avatar link is a public profile projection. User media limits are independent of grants;
`/user get` and `/user set` enforce current access on every selected account.

Message mutation bodies are owned transient rows outside room/private metadata.
The message transaction writes the row, reference changes, counter, retention and
revision together. New attachment-free messages skip reference-table reads;
identity rewrites and edits with unchanged attachment IDs skip reconciliation.
Reactions persist one explicit membership delta without cloning/diffing a full
reaction map; edit rollback coordinates retain only the previous attachment IDs.
Changed reference sets insert/delete only their differences. Startup and failed
command recovery share authoritative metadata/cursor loaders.

Metadata mutation now requires su at execution time. Global admin group bundles
provide only metadata read access; stored direct write flags cannot override the
su check. No metadata rows or existing values are rewritten for this policy change.

`history.clean_min_age` is a typed cleanup limit: `0` disables cleanup, `-1`
is unrestricted, and positive values store minimum message age in seconds.
Room overrides precede global values. Missing values derive from live roles:
admin and su default to `-1`, users to `0`, and room owners to `-1` in their room.
Private cleanup uses the global root. Role defaults are never persisted, so role
revocation and ownership transfer cannot leave stale unlimited values behind.
Su and authorized sudo bypass cleanup permissions and metadata limits.

## Name colors

The optional `color` metadata key names one of 16 fixed palette colors. `@global` holds the
account's default; a stable room ID branch overrides it in that room. Private conversations
use the global value. Writers reject other values, and the key is rejected in private branches. Like
other metadata, colors follow renames and are removed with their room or account. These values
use existing account JSON without layout changes. Policy capabilities use the current v19 schema.

## Avatars

The optional `metadata["@global"]["avatar"]` is an HTTPS image URL or a canonical
`/api/avatars/<id>/file` link. Only this profile key has a self-service HTTP writer;
ordinary metadata mutations still require su. The writer preserves other metadata
and rolls back failed persistence. These values use existing account JSON and
media rows, so color values need no layout change; policy capabilities use the current v19 schema.

Uploaded avatars use media records with conversation `@avatar` and permanent
retention. Immutable file bytes live at `avatars/<stable-account-id>/<upload-id>`
in private S3. Metadata references protect them during startup/daily cleanup. Each cleanup SQL
statement builds one uncorrelated, non-null reference list, avoiding a per-media-row
account JSON scan without changing schema v19. Disabled accounts intentionally keep
those references so their avatar returns on re-enable; their images are unreadable
while disabled. Replacing/removing the avatar or deleting its account releases the
reference.
They count toward existing byte quotas and rate limits; completed avatars do not
consume pending-upload slots. Backups must include the `avatars/` prefix.
