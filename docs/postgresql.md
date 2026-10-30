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
`/system configs`. Setting the URL does not override `database.backend=sqlite`. Selecting
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
must match PostgreSQL schema v1. PostgreSQL is not deployed yet, so fixes are
incorporated directly into v1, including statement-level deletion retention.
There are no historical WIP upgrade paths; unsupported schema versions are
rejected unchanged. SQLite HEAD is v20, with a v19 startup upgrade for explicit bot credential kinds.
The offline transfer requires v20 and preserves issuer/worker credential metadata. See [Data](data.md#database-compatibility).
The PostgreSQL representation has its own schema marker and indexed
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
   HEAD SQLite v20. It checks integrity, foreign keys and metadata before importing.
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
up to eight write leases by default. PostgreSQL also has two to eight read-only history sessions and
a dedicated ownership session. Browser, bot and console operations prepare owned
typed requests under short memory guards, then await native SQLx transactions.
Different conversations can write independently. Metadata administration retains
an exclusive application fence through commit/publication; history deletions use
a separate audit permit. Command effects and retry receipts commit together,
with PostgreSQL receipt checks locked per submitting account before append or
conversation locks. Startup retention/media cleanup and shutdown checkpoints are
native async operations. Shutdown drains accepted I/O/password work first.
Synchronous SQLx utilities remain only for bootstrap/offline transfer, final pool
draining and test compatibility; legacy live command paths are excluded from
production builds.

Optional `database.postgres_pool` settings tune the connection budget:

```json
{
  "database": {
    "backend": "postgres",
    "postgres_pool": {
      "min_readers": 2,
      "max_readers": 8,
      "max_writers": 8,
      "idle_timeout_seconds": 600
    },
    "history_cache": { "max_bytes": "32m", "max_messages": 10000 }
  }
}
```

Reader maximum must be 1–64, reader minimum 0 through that maximum, and writer
maximum 1–32. The dedicated ownership session is additional to these limits.
`idle_timeout_seconds` is 1–86,400 (default 600): idle pooled sessions above the
minimum are eligible for reclamation after this interval. Both PostgreSQL pools
have an explicit 30-minute maximum connection lifetime. The ownership session
never expires or reconnects automatically.
Invalid settings fail before ownership/bootstrap or configuration rewriting.
SQLite ignores PostgreSQL pool tuning and retains its own fixed limits.

These settings take effect after restarting chat. The maximum connection budget
is `max_readers + max_writers + 1` (17 by default); allow additional connections
for administration and backups. The writer pool keeps at least one session, and
the ownership session remains connected even when chat is idle. Reader minimums
control the pool's idle floor, while maximums bound its growth under load.
For a small machine, start with
`"postgres_pool": { "min_readers": 0, "max_readers": 2, "max_writers": 2 }`
for a five-connection ceiling. Lower limits trade concurrent database work for a
smaller connection budget; pool acquisition still times out after five seconds.
Pool sizing does not cap total PostgreSQL or chat memory: PostgreSQL server memory
settings and chat's history/cache limits must be considered separately.

`database.history_cache` applies to both backends and defaults to 32 MiB of
conservative page accounting and 10,000 message entries. `max_bytes` accepts binary
`k`/`m`/`g` units from `4k` to `1g`; `max_messages` accepts 4–100,000. Four shards
each receive one quarter of each budget, rounded down. The fixed 1,000-entry
per-conversation limit, 256-page limit and separate 4,096-entry unread cache still
apply. Response-held pages, metadata, SQLx and PostgreSQL allocations are additional
memory. Recovery clears cached pages while retaining these configured limits.

Su-only `/system configs` shows active configuration as Markdown-fenced JSON.
`/system metrics` shows lifetime counters and effective cache limits, plus
open/idle/minimum/maximum reader and writer sessions,
completed acquisition attempts, errors/timeouts, total and maximum acquisition
microseconds, and whether a dedicated owner exists. Acquisition timing includes
checkout health checks and connection creation; it is not pure queue wait time.
Metrics include history miss/eviction reasons, per-shard usage and unread/snapshot
reuse. Collection uses existing cache/pool paths without periodic profiling or
logging; formatting runs on request. See [Cache metrics](cache.md).
Use `RUST_LOG=chat::engine::database=debug` to record read-operation and write-
transaction durations and success, without credentials, query parameters or bodies.

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
Copying `CHAT_DATA` alone does not back up PostgreSQL chat accounts/history. Stop chat for a
consistent application/database/object-store backup, or coordinate equivalent
snapshots. See [Data](data.md) for the SQLite procedure and [Testing](testing.md)
for isolated PostgreSQL tests.

Read-only ownership checks share a successful result for at most 100 ms and
validate the advisory lock itself. Write acquisition shares cached health; each transaction validates ownership
immediately before commit, and cursor autocommit statements validate immediately
before execution. A probe deadline or known rejected commit is retryable;
ownership loss and unknown commit outcomes require restart to reload memory.
The owning session never reconnects automatically. History pages and unread
batches use one SQL statement each. Page SQL sizes candidate message and compact
reaction payloads before assembling a bounded prefix, avoiding full-page decoding
before the byte budget. Reaction labels are transmitted once per message, with
their complete user lists. The first eligible message is always returned even if
it exceeds the byte budget, preserving progress for large imported records.
Snapshot page loading runs at most four
requests concurrently and retains its message/byte output budgets.
Concurrent viewers share cold page reads and bounded encoded page fragments;
their authorization, cursor and unread projections remain individual.

A known successful commit with a publication mismatch triggers one authoritative
metadata/cursor reload under the exclusive application fence, then invalidates
caches and broadcasts resynchronization. Failed reloads remain unavailable.
This recovery does not clear unknown commit outcomes, ownership loss or poisoned
engine locks; those conditions still require restart.

Keep the offline transfer supported from HEAD SQLite to the current PostgreSQL
schema as PostgreSQL evolves. Source and target schema versions are independent;
the transfer checks the SQLite HEAD version and initializes the current target
schema without changing source bytes. Startup retention reconstruction uses a
transaction-local unlimited statement timeout, leaving runtime deadlines intact.
