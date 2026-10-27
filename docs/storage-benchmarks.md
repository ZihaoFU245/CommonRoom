# Storage benchmarks

## Shared snapshot fan-out

```sh
rtk cargo test --locked snapshot_fanout_shares_cold_reads_and_encoded_content \
  -- --ignored --nocapture
```

This isolated SQLite fixture provisions 100 authorized viewers and 30 rooms,
with three 512-byte messages in each room. Concurrent snapshot assembly and
serialization require exactly 30 cold page reads and reuse the same immutable
pages and encoded ranges. After one append, the next fan-out requires exactly
one page read. Personalized authority, unread state and cursors remain separate.

The review run measured 563 ms for cold fan-out and 487 ms after changing one
room, including assembly and serialization. These are local SQLite fixture
timings, not PostgreSQL throughput or network delivery measurements. Assertions
on read counts, shared storage and one encoding per page are the regression
contract; elapsed time is diagnostic only.

## Current HEAD comparison

```sh
rtk proxy python3 auto/benchmark-storage.py
```

The runner compares the committed HEAD with the working tree using the current
`server/src/engine/tests/benchmarks.rs` fixture. `--baseline-ref COMMIT` selects
another compatible HEAD-era revision. Both versions create fresh isolated temporary
databases; no migration or adaptation for historical database layouts is included.
The baseline export contains the server and compile-time documentation/web fixtures;
it does not include the optional `simpleBot/` worker or working data.

Distinct binary names prevent Cargo from reusing one version's executable for the
other. Both builds share only the dependency/build cache. Python uses `wait4` to
measure each test process's peak RSS on macOS/Linux; compilation is excluded.
The runner never changes the working checkout. Its output reports the resolved
baseline commit so results can be tied to an exact source revision.

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

## Database refactor measurements

One local release-profile run on 2026-10-24 compared committed HEAD
`55e110954048813cd14d6f151edacbd76162778b` with the database refactor in the
working tree. Both fixtures and all timed commands completed successfully.
Latency entries are microseconds; mutation entries show p50 / p95.

| Operation | Small HEAD | Small refactor | Large HEAD | Large refactor |
| --- | ---: | ---: | ---: | ---: |
| Room send | 260 / 401 | 249 / 461 | 247 / 522 | 233 / 464 |
| Room reaction | 89 / 106 | 65 / 90 | 65 / 92 | 65 / 76 |
| Private send | 287 / 509 | 272 / 504 | 466 / 664 | 413 / 562 |
| Private reaction | 93 / 179 | 72 / 101 | 67 / 82 | 65 / 93 |
| Snapshot construction (mean) | 31 | 31 | 608 | 567 |
| Snapshot serialization (mean) | 35 | 35 | 298 | 296 |
| WAL bytes, 120 mutations | 6,245,952 | 6,196,512 | 7,712,672 | 7,778,592 |
| Changed persistent rows | 660 | 660 | 660 | 660 |

Process peak RSS across both fixtures was 28,819,456 bytes for HEAD and
22,822,912 bytes for the refactor. The sample has only 30 observations per
mutation type. Small-room send p95 and large-private reaction p95 increased;
these measurements do not establish a uniform latency improvement. Structural
regressions separately verify bounded mutation writes, skipped attachment queries,
one cold unread reader lease per batch, and no lease for warm unread entries.

## Historical v5/v6 results

The following results were measured on 2026-10-11, Darwin arm64, Rust 1.99.0,
bundled SQLite, release profile. They predate pagination, reader concurrency and
lazy caches, and are retained only as a historical record. They do not measure
current HEAD and cannot be reproduced by the current HEAD-era comparison runner.
Baseline: `181fd92c88bed009f7a67afad393c96b2d7a8bdf` (schema v5).
The compared v6 implementation used targeted persistence for issue #7.

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

## Historical v6 complexity and remaining costs

These costs describe v6. For the current implementation, see
[Architecture](architecture.md) and [Data](data.md).

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

## Historical v6 correctness checks

`auto/check.sh` passes Rust formatting, Clippy, 84 Rust tests (two manual benchmarks
ignored), TypeScript, ESLint, web formatting, and 64 web tests. The debug/release
HTTP/WebSocket integration harness passes privacy, authorization/revocation,
independent retention, monotonic cursors, account deletion/name reuse, restart,
move-folder, and production proxy/cookie checks.
