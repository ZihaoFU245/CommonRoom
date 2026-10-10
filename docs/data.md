# Data

## Data upgrades

Schema v5 automatically migrates v1/v2/v3/v4 data folders, preserving accounts,
sessions, message IDs, replies, mentions, reactions, and existing read cursors.
It adds stable resource/account identities, author IDs, room ownership, scoped
grants, predefined group assignments, and bounded audit records. Existing admin
powers become explicit grants; see [Security](security.md) for the migration policy.
Migration of state, schema version, and legacy read baselines commits together.
A failed migration does not replace the stored state or advance the schema.

Back up the entire data folder with the server stopped before upgrading. Older
binaries cannot open v5. Restore that backup to roll back; there is no downgrade
migration. Tests use isolated temporary data and never modify working `data/`.

Schema v5 renames stored `/permit` and `/unpermit` command grants to `/grant`
and `/revoke`. V4 identities, ownership, scopes, constraints, sessions and read
cursors remain unchanged. Historical audit entries keep the command originally used.

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
the data folder. Only one process can use a folder at once. Compatible newer
binaries read schemas v1/v2/v3/v4 and migrate them to v5 without discarding retained
accounts or messages. Schema v3 added per-conversation private queues and durable
read positions; schema v4 adds grant-based authorization and stable identities; older binaries reject unknown newer schema versions. Browser login sessions keep their original 12-hour expiry across
restart and migration. Accounts, roles, memberships, message timestamps and
IDs remain the same. Proxy certificates, DNS, and OS-specific binaries are
deployment infrastructure and must be supplied on the destination.

On Unix, startup restricts the data directory to its owner (`0700`). This
folder contains private messages and active session credentials; keep
backups private as well.

