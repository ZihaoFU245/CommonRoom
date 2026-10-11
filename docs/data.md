# Data

## Data upgrades

The current schema is v6. New empty databases are initialized as v6. Existing v5 databases migrate automatically in one transaction after validating accounts, policy, and cursor decoding. Schema creation, retained messages/reactions, entities, counters, command-grant updates, and the version change commit together. Failures preserve the v5 state and schema. Existing v1–v4, unversioned, or newer databases are rejected without changing stored state or version.

Back up the entire data folder with the server stopped before upgrading. The v5 → v6 migration preserves accounts, sessions, identities, grants, messages, reactions, reply quotes, conversation revisions, sequences, and read cursors. A v5-only binary cannot open the upgraded folder; restoring the stopped-server backup is the downgrade path. Tests use isolated temporary data and never modify working `data/`.

The command-family rework updates persisted v5 command grants once at startup:
`/delete` and `/deleteuser` become `/rm`, `/user` becomes `/new`, and `/rooms`
and `/users` become `/ls`. Action permissions, scopes, ownership, accounts,
sessions, history, and read cursors are preserved. The policy revision advances
once, and validation or write failures leave the saved state unchanged. This is
included atomically when migrating v5 to v6; old command syntax remains unsupported.

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
the data folder. Only one process can use a folder at once. This binary supports v6 and automatic migration from v5. Browser login sessions keep their original 12-hour expiry across
restart and moving the folder. Accounts, roles, memberships, message timestamps and
IDs remain the same. Proxy certificates, DNS, and OS-specific binaries are
deployment infrastructure and must be supplied on the destination.

On Unix, startup restricts the data directory to its owner (`0700`). This
folder contains private messages and active session credentials; keep
backups private as well.

Account renames use the v6 schema. Account state and renamed account/private read cursor keys commit in one transaction; failed writes preserve the original name and cursors. No new schema migration is required.


## Storage layout and costs

`entities` stores one account, session, room metadata, or private-conversation
metadata per row. `messages` is keyed by stable conversation ID and sequence;
`reactions` stores individual reaction memberships and cascades when a message is
deleted. The small `state` row contains policy/audit data, and `counters` contains
the global sequence. Room/private revisions are separate columns, so a send does
not serialize memberships or policy. Composite tables use `WITHOUT ROWID` to avoid
a redundant row index; cursor cleanup also has a conversation index.

Sends/replies insert one message, optionally delete one oldest message, and update
the counter and conversation revision atomically. A reaction toggles one membership
and updates one revision. Their rollback data holds at most one old message;
unrelated histories are neither cloned nor rewritten. SQLite B-tree lookup/writes
are O(log n), with a constant number of affected rows. Queue append/eviction is
O(1). Message-ID lookup currently scans retained queues; authorization, snapshots,
and broadcasts have their own costs.

Administrative operations, retention-limit changes, cleanup, and retraction can
affect multiple records. They retain full in-memory rollback and reconcile stored
rows transactionally, scanning/serializing retained data but updating only changed
payloads. These operations are not O(1). Startup loads retained history into memory;
this release does not implement lazy history loading. See
[the benchmark report](storage-benchmarks.md) for measured costs and limits.
