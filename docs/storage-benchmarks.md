# Storage benchmarks

Performance fixtures run against isolated temporary data. Set `TMPDIR` to a
writable directory in the checkout to keep their temporary files local. Never
point fixtures at working `data/`. Timing output is diagnostic and depends on
build profile, hardware and load; it is not a production latency guarantee.
Use release builds for performance measurements. Keep structural assertions
about work counts, atomicity and sharing independent of elapsed time.

## Mutation persistence

```sh
rtk cargo test --release --locked mutation_persistence_benchmark -- --ignored --nocapture
```

The SQLite fixture in `server/src/engine/tests/benchmarks.rs` uses 256-byte message
bodies and two workloads: one room/private pair retaining 100 messages each, and
16 room/private pairs retaining 1,000 messages each. Each workload measures 30
room sends, 30 room reactions, 30 private sends and 30 private reactions. Full
conversations evict the oldest message on each send.

Timings surround the test command adapter, which prepares, persists and publishes
through the native operation paths. They include validation and authorization,
but do not simulate concurrent HTTP clients, engine contention or network delivery.
Snapshot construction and serialization are measured separately. The fixture
reports mutation p50/p95, snapshot/serialization means, changed rows and WAL growth.

After fixture creation, WAL is checkpointed and automatic checkpointing is disabled
for the measurement so WAL growth covers all 120 writes. Production retains
SQLite's normal automatic checkpointing. Changed rows come from `total_changes`;
row counts alone do not measure bytes written or memory usage.

## Shared snapshot fan-out

```sh
rtk cargo test --release --locked snapshot_fanout_shares_cold_reads_and_encoded_content \
  -- --ignored --nocapture
```

The SQLite fixture provisions 100 authorized viewers and 30 rooms with three
512-byte messages each. Concurrent snapshot construction and serialization must
perform exactly 30 cold page loads and share immutable pages and encoded ranges.
After appending to one room, native publication refreshes its cached tail, so the
next fan-out performs zero additional page loads. Personalized authorization,
unread state and cursors remain separate. The test asserts sharing and work
counts; reported elapsed time includes assembly and serialization.

## Authorization and scheduling

```sh
rtk cargo test --release --locked permission_lookup_benchmark -- --ignored --nocapture
rtk cargo test --release --locked bounded_page_lookup_benchmark -- --ignored --nocapture
rtk cargo test --release --locked inspection_scheduling_benchmark -- --ignored --nocapture
```

These fixtures exercise compiled permission lookups, worst-position lookup in a
bounded page-cache shard, and uncontended engine inspection versus the blocking
fallback under contention. They report timings without fixed latency thresholds.
See [Caches](cache.md) for budgets and [Architecture](architecture.md) for lock
ownership and the async/blocking boundary.

## PostgreSQL structural checks

Set `CHAT_TEST_POSTGRES_URL` to a disposable PostgreSQL database as described in
[Testing](testing.md). These ignored contracts verify work and correctness rather
than fixed execution times:

```sh
rtk cargo test --locked postgres_bulk_retention_runs_once_per_statement_and_rolls_back \
  -- --ignored --test-threads=1
rtk cargo test --locked postgres_history_bounds_driver_payload_before_decoding_reactions \
  -- --ignored --test-threads=1
```

Bulk deletion must update retention once per statement and preserve transactional
count/oldest metadata on rollback. History queries size candidate message/reaction
payloads before assembly and select a bounded prefix. A single oversized message
is allowed for pagination progress. Tests check raw payload bounds, complete
reactions and both pagination directions. SQL payload size does not measure process
peak RSS; pool buffers, caches and response-held pages also consume memory.
