# Historical v6 mutation persistence benchmarks

These recorded results predate v7 pagination, reader concurrency and lazy caches. They are not measurements of the current implementation. The runner now seeds both versions through public commands and adapts fixture access to their different storage ownership.

Measured on 2026-10-11, Darwin arm64, Rust 1.99.0, bundled SQLite, release profile.
Baseline: `181fd92c88bed009f7a67afad393c96b2d7a8bdf` (schema v5).
Current: v6 targeted persistence implemented for issue #7.

## Reproduce

```sh
rtk proxy python3 auto/benchmark-storage.py
```

The runner archives the baseline into a temporary directory and supplies the
identical benchmark source to both versions. Separate binary target names prevent
Cargo from reusing one version's executable for the other. Both builds share only
the dependency/build cache. `--baseline-ref COMMIT` overrides the baseline. No
working data or Git checkout is modified. Python uses `wait4` to measure each test
process's peak RSS; compilation is excluded.

Two fixtures contain 256-byte message bodies:

- Small: one populated room and one private pair, each retaining 100 messages.
- Large: 16 populated rooms and 16 private pairs, each retaining 1,000 messages
  (32,000 retained messages total).

Each fixture performs 30 room sends, 30 room reactions, 30 private sends, and 30
private reactions. Full queues evict the oldest message on every send. Timings use
`Instant` around `Engine::execute`, including validation, authorization, rollback
preparation, and persistence. This is the synchronous work inside the production
engine critical section; it does not simulate lock contention or concurrent clients.
The benchmark separately measures 30 snapshot constructions and 30 serializations
of one snapshot. Neither is included in mutation timing. For broadcasting, each
recipient adds its own authorization-filtered snapshot and serialization; actual
network queue/send time is not measured here.

WAL is checkpointed after fixture creation, then auto-checkpointing is disabled
**only for the benchmark** so WAL growth captures all 120 mutation writes instead
of just the final checkpoint interval. Production keeps SQLite's default automatic
checkpointing. Changed-row counts come from `total_changes`; more normalized rows
can still produce far less write volume than replacing one giant JSON row.

## Recorded results

All latency entries are microseconds, expressed as p50 / p95. Small-sample timing
noise explains why some small-fixture current results exceed the large fixture.
These are measured values, not service latency guarantees.

| Operation | Small v5 | Small v6 | Large v5 | Large v6 |
| --- | ---: | ---: | ---: | ---: |
| Room send | 366 / 723 | 159 / 416 | 30,148 / 33,492 | 84 / 106 |
| Room reaction | 396 / 630 | 113 / 183 | 30,115 / 34,118 | 60 / 80 |
| Private send | 353 / 604 | 158 / 262 | 30,068 / 33,462 | 88 / 127 |
| Private reaction | 370 / 592 | 119 / 162 | 30,296 / 33,638 | 62 / 84 |
| Snapshot construction (mean) | 64 | 92 | 485 | 487 |
| Snapshot serialization (mean) | 45 | 63 | 436 | 435 |
| WAL bytes, 120 mutations | 13,126,352 | 1,763,392 | 1,979,602,352 | 2,224,832 |
| Changed persistent rows | 120 | 360 | 120 | 360 |

Whole-benchmark peak RSS was 1,166,131,200 bytes for v5 and 83,607,552 bytes for
v6. This includes fixture setup, retained state, administrative rollback during
setup, SQLite caches/temp tables, and uncheckpointed WAL handling. It is not a
per-message allocation measurement or a prediction of production RSS. The hot
rollback structure itself stores at most one old message and a conversation key,
sequence, revision, and optional index; it has no retained-history-sized clone or
serialized buffer. Message serialization borrows the payload.

The large-fixture send median improves about 359× and reaction median about 502×;
WAL growth improves about 890×. Snapshots/serialization remain essentially unchanged.
This evidence justifies moving frequent mutation persistence out of the whole-state
JSON representation.

## Complexity and remaining costs

- Appending/evicting from a conversation deque is O(1).
- A send/reply performs one message insert, at most one eviction, one counter
  update, and one conversation-revision update. Creating a private pair adds one
  metadata row. Eviction cascades any reaction memberships attached to that message.
- A reaction changes one membership row and one revision, without rewriting the
  message text or unrelated history. Reaction/member collections remain bounded
  by the configured user capacity and per-message reaction limit.
- SQLite B-tree operations are O(log n), not literal O(1). Composite tables use
  `WITHOUT ROWID` to avoid storing a duplicate row index. No new in-memory
  per-message lookup map or permanent duplicate persistence cache is introduced.
- Message-ID lookup for reactions/replies/retractions still scans retained queues;
  this benchmark targets the newest message in `room0`/`peer0`, not worst-case
  message lookup. Authorization and private conversation discovery also have costs.
- Cleanup, retraction, account/room changes, login/session operations, and retention
  limit changes retain whole-state rollback and transactional reconciliation. They
  can scan/serialize retained records even though unchanged persistent payloads are
  not rewritten. They are not O(1), and their latency is not measured here.
- Startup loads all retained history. Snapshots compute unread state and construct
  per-recipient output; broadcast fan-out and mutex contention are separate scaling
  limits. Lazy history loading, all-operation mutation journals, and constant-time
  in-memory message lookup are not part of this change.

## Correctness checks

`auto/check.sh` passes Rust formatting, Clippy, 84 Rust tests (two manual benchmarks
ignored), TypeScript, ESLint, web formatting, and 64 web tests. The debug/release
HTTP/WebSocket integration harness passes privacy, authorization/revocation,
independent retention, monotonic cursors, account deletion/name reuse, restart,
move-folder, and production proxy/cookie checks.

At the time of these historical measurements, migration tests preserved v5 accounts, identities, ownership, policy, sessions,
messages/reactions/replies and read positions across upgrade and restart. Policy
validation, cursor decoding, and injected storage failures left the v5 version and
schema intact. Targeted-write tests reject unrelated payload/policy writes and check
exact row counts. Failure injection after insertion, eviction, counter/revision
writes, reaction changes and private-pair creation verifies disk/cache rollback.
