# Data

## Database compatibility

The supported schema is v8, matching master HEAD. Empty databases are initialized
transactionally as v8. Existing v8 databases reopen without migration or command-grant
rewrites. All older, newer and existing unversioned databases are rejected before
changing SQLite journal settings, schema, state or version. No historical schema
migration is included in this binary.

Back up the entire data folder with the server stopped. Tests use isolated temporary
data and never modify working `data/`.

## Move to another machine

1. Stop the old server with Ctrl + C or SIGTERM. Wait for it to exit.
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
    │                    # messages, read positions, and unexpired login sessions
    └── chat.lock        # exclusive OS lock; safe to move after shutdown
```

SQLite may also have `chat.sqlite-wal` and `chat.sqlite-shm`; copy the whole
folder, never just the database while it is running. The stored format uses
portable SQLite and JSON, with no machine-specific paths or secrets outside
the data folder. Only one process can use a folder at once. This binary supports only v8. Browser login sessions keep their original 12-hour expiry across
restart and moving the folder. Accounts, roles, memberships, message timestamps and
IDs remain the same. Proxy certificates, DNS, and OS-specific binaries are
deployment infrastructure and must be supplied on the destination.

On Unix, startup restricts the data directory to its owner (`0700`). This
folder contains private messages and active session credentials; keep
backups private as well.

Account renames use the current schema. Account state and renamed account/private read cursor keys commit in one transaction; failed writes preserve the original name and cursors. No new schema migration is required.


## Storage layout and costs

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
This additive field uses the existing v8 storage schema. Edits update the
message payload and conversation revision in one transaction; failed writes
roll back both. Existing reply quotes retain the text captured when replying.
