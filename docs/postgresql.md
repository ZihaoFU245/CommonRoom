# PostgreSQL and SQLite

Select the backend explicitly in the `database` section of `data/config.json`.
Existing configurations without this section default to SQLite and are normalized
to include `{"database":{"backend":"sqlite"}}` on startup. For PostgreSQL, set:

```json
{
  "database": {
    "backend": "postgres"
  }
}
```

Merge this section into the existing configuration, keeping other settings. Supply
the PostgreSQL connection URL separately through the environment:

```sh
export CHAT_DATABASE_URL='postgresql://dev:devpass@127.0.0.1:5432/devdb'
rtk cargo run --locked
```

The URL is environment-only: it is not saved in `config.json` or shown by
`/configs`. Setting the URL does not override `database.backend=sqlite`. Selecting
`postgres` without a nonempty URL fails startup without falling back to SQLite.
S3 access keys, secret keys and session tokens also remain environment-only;
non-secret S3 endpoint/bucket/region settings stay in the existing `s3` section.
Unknown database fields (including `url` and `password`) are rejected. PostgreSQL 18 is the tested server version. The URL accepts ordinary
PostgreSQL connection parameters; use `sslmode=verify-full` and a trusted server
certificate for remote production connections. Local containers without TLS use
the driver's default SSL preference or an explicit `sslmode=disable`.

Both database drivers use SQLx. PostgreSQL TLS uses rustls with its ring provider,
so the database driver does not require platform OpenSSL libraries. Private CAs
can be supplied with the PostgreSQL `sslrootcert` connection parameter.

Use a dedicated database with an empty target schema on first start. The role
needs permission to create tables, indexes, sequences and trigger functions there.
Startup initializes an empty schema transactionally; an existing application schema
must be the current version, v19. Existing unversioned objects and other schema
versions are rejected without modifying them. SQLite's existing v19 file layout
is unchanged. The PostgreSQL representation has its own schema marker and indexed
JSONB projections alongside the original JSON text; the two database files/formats
are not interchangeable.

`CHAT_DATA` still selects the local folder for `config.json` and independently
managed worker files. A PostgreSQL server does not open or synchronize
`CHAT_DATA/chat.sqlite`. Keep `database.backend=postgres` in the configuration and
the URL in the service environment on every start after cutover. Returning to
SQLite requires explicitly selecting `database.backend=sqlite`.

## Local development server

With PostgreSQL installed and its `pg_config` on `PATH`, run:

```sh
rtk proxy ./auto/start-postgres.sh
```

This initializes `./pgdata/` on first use with role `cr`, password `pass`, and
an empty `devdb` database, then listens on `127.0.0.1:5432`. In another terminal,
run `rtk proxy ./auto/debug.sh --with-postgres`. Debug only connects; it does not
manage PostgreSQL. Stop the launcher with Ctrl+C; later runs reuse the cluster.
Do not run another PostgreSQL server on the same port.

The cluster, WAL, configuration, `postgres.log`, Unix socket, and temporary files
stay under `pgdata/`. The helper clears inherited PostgreSQL environment settings
and confines filesystem writes to that directory (macOS `sandbox-exec`, or Linux
with `bubblewrap` installed). It rejects a symlink for `pgdata/`. No Homebrew service
or system cluster is started or modified. `auto/clean.sh` preserves `pgdata/`.
Back up this cluster using the PostgreSQL procedures below; do not copy a running
cluster as ordinary files. These fixed credentials are for loopback development.

## Transfer an existing HEAD SQLite database

The built-in offline transfer needs no pgloader installation. Stop chat first and
back up the entire local data folder and the object store consistently. Choose an
empty PostgreSQL database/schema; do not start chat against it before migrating.

From the repository, with the PostgreSQL container already running:

```sh
rtk cargo build --locked
export CHAT_DATABASE_URL='postgresql://dev:devpass@127.0.0.1:5432/devdb'
rtk proxy ./target/debug/chat --migrate-sqlite ./data/chat.sqlite
```

After a successful transfer, change `database.backend` to `postgres` in
`data/config.json`, then start chat with the URL still set:

```sh
rtk proxy ./target/debug/chat
```

For a packaged release, use the new `./chat` binary in place of
`./target/debug/chat`. Preserve the existing `CHAT_DATA`, hostname, base path and
S3 settings. Session credentials retain their original expiry; there is no password
reset, account recreation or message renumbering.

The command:

1. Acquires the SQLite server's ownership lock and opens the source read-only at
   HEAD v19. It checks integrity, foreign keys and metadata before importing.
2. Acquires PostgreSQL ownership and requires an empty target schema.
3. Creates the PostgreSQL schema and streams base rows using binary `COPY` inside
   one transaction. It retains accounts/password hashes, sessions, policy, bots,
   message JSON, reactions, revisions, global sequences, read positions, both
   kinds of retry receipts, audit/authentication events and media metadata.
4. Rebuilds account references and retained-message counters using PostgreSQL
   triggers, and compares row counts plus typed, ordered checksums for all 14
   application tables. Restores authentication/audit identity sequences.
5. Commits only after verification, then prints per-table verified row counts.

Failure before commit rolls the new target schema/data back. The source database
is not rewritten. A nonempty target is rejected; the command does not merge,
overwrite, incrementally synchronize or upgrade old schemas. The explicit migration
command uses the destination URL without changing the saved backend selection. After a successful
transfer, verify login, room/DM history, replies/reactions, unread positions and
attachments with the new server before retiring the old deployment.

Object bytes remain in the configured S3-compatible bucket; the transfer copies
their metadata/references, not the objects. `config.json`, the reference worker's
checkpoint/outbox, SimpleBot's Deno KV and workspace files stay where they are.
Keep them and their own backups when moving machines.

The preserved SQLite backup is a rollback point from before cutover. Once new
writes reach PostgreSQL, switching back to it loses those new writes. Reverse
transfer and online replication are not implemented.

## Runtime and backups

Both backends use SQLx. The typed async dispatcher handles message transactions
and cursor advances with one SQLite writer lease or a bounded PostgreSQL pool of
up to eight write leases. PostgreSQL also has four read-only history sessions and
a dedicated ownership session. Existing synchronous engine entry points bridge
to the dispatcher; broader backend SQL extraction and removal of engine-wide
mutation serialization remain in progress. The pool supports independent writes,
but this does not yet make all chat requests concurrent.

Message operations validate the stored conversation identity/revision under a
PostgreSQL row lock. Appends first lock the global counter row and validate their
expected sequence, retaining current global ordering through commit. Consequently,
appends still have a global serialization point; edits/reactions to independent
conversations and independent cursor rows can use separate write sessions.

Normal PostgreSQL statements have a 10-second server deadline and a 2-second lock
deadline. Offline transfer disables the request deadline for its own connection
while streaming and checking data. Read-only history transactions use repeatable
read for coherent multi-statement history/unread reads.

Only one chat process may own a PostgreSQL database. A dedicated session advisory
lock enforces database ownership; it is never held on a pool lease. The local
`chat.lock` also protects the configuration folder. Ownership is acquired before
configuration normalization and checked independently of writer-pool capacity.
Losing ownership or an active session/uncertain commit invalidates the dispatcher
and requires a restart. SQLx may replace an idle pool connection safely while the
ownership session remains healthy. Use a direct connection or session pooling;
transaction pooling cannot preserve the ownership lock. Engine state, presence,
caches and notifications remain single-process; this does not enable replicas or
change the configured user/room/retention ceilings.

For PostgreSQL backups use PostgreSQL's `pg_dump`/`pg_restore` or managed physical
backup tooling, alongside local configuration/worker backups and the object store.
Copying `CHAT_DATA` alone no longer backs up chat accounts/history. Stop chat for a
consistent application/database/object-store backup, or coordinate equivalent
snapshots. See [Data](data.md) for the SQLite procedure and [Testing](testing.md)
for isolated PostgreSQL tests.
