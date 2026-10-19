# Data

## Database compatibility

Validated v9/v10/v11/v12/v13/v14 databases upgrade transactionally to v15. V9 first gains native
command receipts. V11 adds a reverse index of author, recipient, reply-author and
mention names, maintained by message insert/update/delete operations, plus a
reaction-user index. Existing retained history is indexed once during migration.
Failed migration rolls back all schema/version changes. Back up before upgrading;
older binaries cannot open v15.

V12 moves the bounded audit log to append-only `audit_entries` records and adds a
reply-target index. Migration copies embedded audit entries before removing them
from the policy state row; retention prunes only records beyond the latest 1,000.

V13 adds media metadata and message references with cascading deletion.

Empty databases initialize as v15. Existing v15 databases reopen without migration.
History, message receipts, accounts, sessions and read cursors are preserved.

The removed `x:group.su.assign` permission is accepted only in existing stored
grants and supplies no authority. Those entries are preserved and can be removed
with `/revoke user scope x:group.su.assign`; new grants of this name are rejected.
Su group assignment still requires su authority, including authorized `/sudo`.
That permission-catalog change itself requires no data rewrite.

Schemas other than v9/v10/v11/v12/v13/v14/v15 and existing unversioned databases are rejected before
changing journal settings, schema, state or version. Do not lower `user_version`
manually on a working database.

Back up the entire data folder with the server stopped. Stop any reference agent
writing its checkpoint there too. Tests use isolated temporary data and never
modify working `data/`.

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
the data folder. Only one process can use a folder at once. This binary reads v15 and upgrades v9/v10/v11/v12/v13/v14. Browser login sessions keep their original 12-hour expiry across
restart and moving the folder. Accounts, roles, memberships, message timestamps and
IDs remain the same. Proxy certificates, DNS, and OS-specific binaries are
deployment infrastructure and must be supplied on the destination.

On Unix, startup restricts the data directory to its owner (`0700`). This
folder contains private messages and active session credentials; keep
backups private as well.

Account renames use the v11 reverse name index. Account state and renamed account/private read cursor keys commit in one transaction; failed writes preserve the original name and cursors. The index is backfilled by the v11 migration.

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

Sends/replies insert one message, prune beyond the configured per-conversation retention, and update counter/revision atomically. A reaction changes membership rows and one revision. SQL range lookups use the conversation/sequence primary key. Cached pages are keyed by stable conversation identity, revision and range, so reaction/cleanup/rename changes cannot reuse stale content.

Startup loads metadata and read positions only. Bodies and reactions are loaded in pages of at most 100. One writer connection and four independent read-only connections use WAL; concurrent history reads do not hold the engine writer lock. Cache eviction has no persistence effect. Server pages are bounded to 1,000 entries per conversation, 10,000 total, and 32 MiB of conservative payload accounting; SQLite page caches and response copies are additional memory.

Account rename/deletion reads only indexed references in batches of at most 100 message records inside the atomic transaction with session/cursor changes. Cleanup and retention pruning remain transactional and may process many disk rows, but no longer clone all history in memory. Administrative rollback retains metadata only. The old v6 measurements in [Storage benchmarks](storage-benchmarks.md) are historical and do not establish current throughput.

Message JSON may include a boolean `markdown` field. Missing fields default to
false, preserving plain-text rendering for existing messages. This additive JSON
field does not change the SQLite schema or require a migration.

Message JSON may include an `edited` boolean. Missing values default to false.
This additive field uses the existing message storage. Edits update the
message payload and conversation revision in one transaction; failed writes
roll back both. Existing reply quotes retain the text captured when replying.

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

## Command retry receipts (v10)

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
independently of server SQLite schema v15 and the minimal reference worker.
Deno runs it with `--unstable-kv`. Old `checkpoint.json` and `BOT_STATE` are
ignored; no legacy checkpoint migration is performed.

KV format v2 stores account binding, input scan cursors, revisions, accepted
message versions, tasks, tick sessions/continuations, durable jobs, command
outcomes and the delivery outbox. Opening validated v1 worker data upgrades its
format marker atomically while retaining existing tasks, payloads and retry
keys; unsupported versions are rejected. This does not change server schema v15.
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

## Media objects (v13-v14)

`media` stores pending/ready object metadata and stable account/conversation IDs.
`message_media` references `(conversation,sequence)` and cascades on message
deletion. References are reconciled in each message transaction, including edits;
failed writes roll back both. Media metadata stays in SQLite, bytes in a private
S3-compatible bucket. Back up and restore **both** together when uploads are enabled.
See [Media uploads](media-uploads.md) for lifecycle, cleanup and deployment.

V14 adds indexed expiry, publication, deletion/tombstone and cleanup-attempt state.
V13 upgrades transactionally; referenced legacy objects expire seven days after
creation and retain their existing object keys. Unreferenced uploads use finite
pending cleanup. Typed media constraints on grants are defaulted JSON fields;
old grants retain their original meaning. See [Media uploads](media-uploads.md).

Room media-upload restrictions are stored by stable account/room IDs in policy
JSON's defaulted `media_revocations` map. Existing room user/admin assignments gain
default uploads at policy compilation. Restrictions commit with audit/revision
updates, survive renames and owner transfers, and are removed on account/room
deletion. This additive JSON field requires no SQL schema change.

V15 adds media cleanup partial/covering indexes, `staging_attempt`, and a
reference-removal trigger. A v14 upgrade backfills orphan timestamps once inside
the migration transaction; cleanup no longer scans every live object to find
orphans. Existing media JSON remains compatible, but ready/expiry/deletion
columns are authoritative when loading. Downgrading after migration is unsupported.
