# Data

## Data upgrades

Only the current HEAD schema (v5) is supported. New empty databases are initialized as v5; existing v1–v4, unversioned, or newer databases are rejected without changing their stored state or version. Legacy upgrade code has been removed.

Back up the entire data folder with the server stopped before upgrading. Current v5 accounts, sessions, identities, grants, messages and read cursors remain compatible. Tests use isolated temporary data and never modify working `data/`.

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
the data folder. Only one process can use a folder at once. This binary supports only schema v5. Browser login sessions keep their original 12-hour expiry across
restart and moving the folder. Accounts, roles, memberships, message timestamps and
IDs remain the same. Proxy certificates, DNS, and OS-specific binaries are
deployment infrastructure and must be supplied on the destination.

Agent accounts, their provider keys, their optional web-search keys, and their
personality text are stored in the same state document, so they move with the
folder and survive a restart. Agents are additive: a folder
written before agents existed loads unchanged, and accounts then default to
ordinary people in `mention` mode. A provider key is a credential: keep the
data folder and its backups private, and rotate a key with `/agent-key` if it is
exposed. Renaming an agent rewrites its memberships, its retained message
labels, and its private-conversation keys, which is what keeps a rename
consistent across a restart.

On Unix, startup restricts the data directory to its owner (`0700`). This
folder contains private messages and active session credentials; keep
backups private as well.

Account renames use the existing v5 schema. Account state and renamed account/private read cursor keys commit in one transaction; failed writes preserve the original name and cursors. No new schema migration is required.
