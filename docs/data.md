# Data

## Database compatibility

The current schema is v9. Empty databases initialize transactionally as v9.
Existing v9 databases reopen without migrations. Older schemas are unsupported.

The removed `x:group.su.assign` permission is accepted only in existing stored
grants and supplies no authority. Those entries are preserved and can be removed
with `/revoke user scope x:group.su.assign`; new grants of this name are rejected.
Su group assignment still requires su authority, including authorized `/sudo`.
This catalog change does not change the v9 schema or require a migration.

Schemas other than v9 and existing unversioned databases are rejected before
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
the data folder. Only one process can use a folder at once. This binary reads only v9. Browser login sessions keep their original 12-hour expiry across
restart and moving the folder. Accounts, roles, memberships, message timestamps and
IDs remain the same. Proxy certificates, DNS, and OS-specific binaries are
deployment infrastructure and must be supplied on the destination.

On Unix, startup restricts the data directory to its owner (`0700`). This
folder contains private messages and active session credentials; keep
backups private as well.

Account renames use the current schema. Account state and renamed account/private read cursor keys commit in one transaction; failed writes preserve the original name and cursors. No new schema migration is required.

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
deleted. The small `state` row contains policy/audit data, and `counters` contains
the global sequence. Room/private revisions are separate columns, so a send does
not serialize memberships or policy. Composite tables use `WITHOUT ROWID` to avoid
a redundant row index; cursor cleanup also has a conversation index.

The current schema includes message-ID, author/sequence, time, global sequence and conversation-ID indexes.

Sends/replies insert one message, prune beyond the configured per-conversation retention, and update counter/revision atomically. A reaction changes membership rows and one revision. SQL range lookups use the conversation/sequence primary key. Cached pages are keyed by stable conversation identity, revision and range, so reaction/cleanup/rename changes cannot reuse stale content.

Startup loads metadata and read positions only. Bodies and reactions are loaded in pages of at most 100. One writer connection and four independent read-only connections use WAL; concurrent history reads do not hold the engine writer lock. Cache eviction has no persistence effect. Server pages are bounded to 1,000 entries per conversation, 10,000 total, and 32 MiB of conservative payload accounting; SQLite page caches and response copies are additional memory.

Account rename/deletion streams at most 100 message records at a time inside the atomic transaction with session/cursor changes. Cleanup and retention pruning remain transactional and may process many disk rows, but no longer clone all history in memory. Administrative rollback retains metadata only. The old v6 measurements in [Storage benchmarks](storage-benchmarks.md) are historical and do not establish current throughput.

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
