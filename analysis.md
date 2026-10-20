# Media / S3 analysis: efficiency, correctness, and code quality

Scope: the two commits that added private S3 uploads and native playback
(`fcc780b`, `ef5749e`). The document collects three kinds of findings:
correctness and robustness (§11), efficiency and optimization (the P0–P3 table
and §1–§10), and code quality and redundancy (§12). The priorities are
suggestions, not a commitment.

Assumptions used for the estimates:

- one server process, one SQLite connection behind a mutex, 64 users / 64 rooms
  (the defaults; the ceilings are 256 users / 1024 rooms), `max_size` 25 MiB, `user_quota` 1 GiB,
  `pending_limit` 8, `allocations_per_minute` 30;
- S3 same-region RTT ≈ 10 ms, cross-region RTT ≈ 100 ms (all numbers below are
  RTT-dominated; the code itself is cheap);
- media objects are immutable once `complete` succeeds, so caching is safe
  except for the revocation/expiry window.

## Verification and fixes

Verified against the implementation after `ef5749e`. The original review below
is retained as historical input; these notes supersede its estimates and advice.

- C1/C2: confirmed and fixed. API permits now cover blocking auth/SQL jobs only.
  Eight independent S3 slots bound outbound work; per-object locks replace the
  global gate. Duplicate completion fails promptly with 503 rather than creating
  an unbounded queue. Different objects complete concurrently.
- C3/C4/Q10: fixed typed 400/503 completion errors, zero-size guard, bounded range
  streaming, and storage/signing diagnostics. Finalization now uses three calls;
  the copied object's Content-Range proves size without another HEAD.
- P1 probing: overlapping callers share the in-flight result. There is no health
  TTL cache: a later request still probes and rejects an offline bucket before
  reserving quota, preserving the agreed behavior.
- P1/P2/Q4/Q6/Q7/Q9: fixed with v15 migration, covering quota and partial cleanup
  indexes, orphan tracking on reference deletion, bounded candidate selection,
  canonical mutable SQL columns, combined metadata/reference reads, and a no-op
  reference fast path. Migration and query-plan tests use isolated databases.
- P2 cleanup: four concurrent object jobs, deduplicated/batched object deletes,
  per-key success checking, fallback for unsupported batch APIs, retry ordering
  for both final and staging objects, Skip missed ticks, and a 20-second sweep
  budget. Full batches drain until short or the time budget expires. No jitter
  is needed for this explicitly single-process deployment.
- Q1/Q3/Q5/Q8/Q11/Q12: removed duplicate policy check, shared response/auth job
  helpers, reused resolved scope, distinguished quota/rate/pending errors, logged
  no-op abandonment, typed attachment kinds, shared path validation, and handled
  Markdown construction failure without losing the draft.
- Client: progress is deduplicated and limited to 10 Hz, object URL previews are
  capped at 4 MiB, completion has a 100-second server budget under the 120-second
  client deadline, and PUT has a separate 15-minute transfer timeout.
- C5: localhost CORS and production CSP/nosniff prerequisites remain explicit
  operator configuration. Automatically widening origins would be incorrect.
- C6/Q13: existing default retention and live default grant constraints are
  documented policy, not defects. Preserve `-1` and the wire/storage contracts.
  The timestamp wrapper intentionally exposes the private engine clock across
  the module boundary. The access flag and allowance expression were clarified.
- C7: abandonment of published/deleted rows intentionally succeeds; now tested
  and logged at debug level.
- Caching was not enabled: `no-store` preserves current access/retraction and
  private-content caching semantics. A redirect/byte cache is a policy change,
  not a correctness fix. Native playback does not necessarily return to chat
  for every range request; browsers commonly reuse the redirected S3 URL.
- Size/expiry correction: AWS documents a 5 GB single PUT/copy limit; the exact
  decimal byte claim in section 7 is not established there. Configuration now
  uses a conservative portable 5,000,000,000-byte ceiling. URL expiry is checked
  at request admission; it is not an in-flight transfer stopwatch. The previous
  300-second XHR timeout, not URL expiry, enforced that client transfer window.
  See https://docs.aws.amazon.com/AmazonS3/latest/userguide/using-presigned-url.html
  and https://docs.aws.amazon.com/AmazonS3/latest/API/API_CopyObject.html.
- The 1-second configurable cleanup floor is retained for explicit development
  and operational use. Work remains bounded by dedicated S3 admission and the
  sweep deadline; silently imposing 60 seconds would change supported settings.
- Q14: regression coverage includes slow S3 isolation, per-file concurrency,
  status mapping, filename encoding, migrations/rollback, orphan tracking,
  canonical columns, no-op abandon/reconcile, and frontend URL/preview bounds.
  Runtime spans/logs and deterministic delayed-bucket tests replace hypothetical
  latency calculations; no synthetic QPS claims are made.

## TL;DR

| # | Change | Why it matters | Effort |
|---|--------|----------------|--------|
| P0 | Stop holding the shared 64-permit `io_jobs` semaphore across S3 awaits; acquire it only around `spawn_blocking` DB work, and add a small dedicated S3 semaphore | one stalled bucket can turn into a full-API 503 outage (`server/src/web/media.rs:59`, `:123`, `server/src/web/mod.rs:53`) | S |
| P0 | Replace the global `storage.gate` mutex with a per-media lock (keep the DB claim in `expired_media`) | all completions and the whole cleanup currently serialize; cleanup can hold the gate for minutes (`server/src/web/media.rs:125`, `:238`) | M |
| P1 | Cache the 307 redirect briefly (`private, max-age=30`) and let S3 cache bytes (`response-cache-control: private, max-age=60`) | every image view today costs a chat request + 4 SQLite reads + a fresh presign; repeat views refetch bytes because URLs are always unique (`server/src/media.rs:190`, `server/src/web/security.rs:54`) | S |
| P1 | Overlap/cache the `check_reachable` probe, and cut `finalize` from 4 to 3 S3 calls | saves 1 RTT on every upload (cache) and 1 RTT + 1 request per completion (`server/src/media.rs:124-165`) | S |
| P1 | Add partial indexes for cleanup and make the quota aggregate index-only (or maintain counters) | cleanup currently scans/sorts all live rows every 5 min; allocation scans all of a user's rows (`server/src/engine/storage/media.rs:135`, `:54`) | M |
| P1 | Fix the upload ceiling: config allows 5 GiB but a single presigned PUT and `CopyObject` both stop at 5 GB, and the PUT URL lives 300 s | `max_size` above 5,000,000,000 can never succeed; 25 MiB needs ≥ 0.67 Mbit/s to fit in 300 s (`server/src/config.rs:68`, `server/src/media.rs:120`) | S |
| P2 | Batch `DeleteObjects`, drop the guaranteed-404 legacy delete, drain cleanup batches with a deadline, skip missed ticks + jitter | cleanup issues 2–3 DELETE per media, one of which always 404s (`server/src/web/media.rs:243-276`, `server/src/web/media.rs:217`) | M |
| P2 | Merge `stored_media` + `media_attached` into one query, return the checked `Media` from the pre-check into `finish_media`, skip `reconcile`'s no-op DELETE for media-free edits | halves SQLite round trips on the read path; removes duplicate reads on complete | S |
| P2 | Throttle XHR progress state, align client/server timeouts, cap preview blob size | removes per-progress React renders and duplicate completions after 120 s | S |
| P3 | Add spans/counters (`allocate/complete/download/cleanup` duration, 503s, cleanup backlog, per-request S3 call count) and a load test with a latency-injected bucket | the current code has only `tower_http` request logs; the P0/P1 claims are easy to verify and easy to regress | M |

## Issue index

Correctness and robustness (§11):

- **C1 (HIGH)** — the shared 64-permit `io_jobs` semaphore is held across S3
  awaits, so a stalled bucket or a full gate queue returns 503 for every
  endpoint, including logins and history.
- **C2 (HIGH)** — one global `storage.gate` serializes every completion and the
  whole cleanup; a slow cleanup can hold it for minutes.
- **C3 (MEDIUM)** — `complete` maps storage/finalize failures to 400, so
  transient S3 errors look like client errors and are not retried as 5xx.
- **C4 (LOW)** — `media.size.min(4096) - 1` underflows for a zero-size row in
  `finalize`; the engine rejects zero-size allocations, but the transport layer
  should still guard.
- **C5 (LOW)** — operator prerequisites are easy to miss: local bucket CORS only
  allows port 5173, nginx CSP has no storage origin, and signed GETs rely on the
  bucket sending `nosniff`.
- **C6 (NIT)** — `su` needs an explicit `-1` grant to upload permanent media;
  grants with omitted limits resolve against the config at request time, not at
  grant time; the implicit private allowance also requires a group.
- **C7 (NIT)** — `abandon` returns success when the row was already published or
  deleted (zero-row update).

Code quality and redundancy (§12):

- **Q1** — duplicate `require_target_command` call in the media grant/revoke
  branch (`server/src/engine/authorization/commands.rs:250`, `:259`).
- **Q2** — upload path validates the same allocation/ownership twice
  (`server/src/web/media.rs:71` + `server/src/engine/media.rs:201`; `:132` +
  `:256`).
- **Q3** — the media response JSON is duplicated in `complete` and `info`
  (`server/src/web/media.rs:152`, `:171`).
- **Q4** — the tombstone DELETE is duplicated verbatim
  (`server/src/engine/storage/media.rs:139`, `:192`) and `now()` is re-read
  several times per sweep.
- **Q5** — `media_scope` resolves the conversation, then callers resolve it
  again (`server/src/engine/media.rs:153-183`, `:241-246`).
- **Q6** — `stored_media` + `media_attached` take two locks and issue two
  queries on the read path (`server/src/engine/media.rs:246-248`, `:264-271`).
- **Q7** — `reconcile` always deletes and reinserts `message_media`, even for a
  text-only message with no media (`server/src/engine/storage/media.rs:34-50`).
- **Q8** — six handlers repeat origin check + permit + `spawn_blocking` auth
  (`server/src/web/media.rs:32,51,115,157,180,281`); extractors would make the
  permit scope explicit and remove boilerplate.
- **Q9** — object state lives in both columns and the JSON blob and is patched
  at four sites (`server/src/engine/storage/media.rs:87,121,139,187`); a missed
  mirror is silent drift.
- **Q10** — `String` errors discard the S3 cause, and presign failures are not
  logged (`server/src/media.rs:124-219`;
  `server/src/web/media.rs:106-109`, `:202-205`).
- **Q11** — `abandon_media` is silent on a zero-row update, and
  `store_media_allocation` uses one message for quota/pending/rate
  (`server/src/engine/storage/media.rs:121-131`, `:60-66`).
- **Q12** — client duplication and loose typing: unused `expires_in` validation
  (`web/src/api/media.ts:7,33`), `kind: string` (`web/src/components/Attachment.tsx:10`),
  `mediaMarkdown` can throw inside `send` (`web/src/api/media.ts:65`), and path
  parsing exists twice (`:47-60`, `:137-160`).
- **Q13** — readability: boolean `write` flag, `-1` sentinel, long
  `media_allowance` expression, and the `timestamp()` wrapper
  (`server/src/engine/media.rs:25-27,79-124,153`).
- **Q14** — test gaps: filename encoding in `download_url`, `attachmentLink`
  base paths, zero-row abandon, reconcile no-op, and the 400-vs-503 mapping.

## Cost per operation today

Counts are server-side. "RTT" = S3 round trip; presigning is local CPU.

| Operation | Chat HTTP | S3 requests | SQLite statements | Sequential RTTs |
|-----------|-----------|-------------|-------------------|-----------------|
| `POST /api/media/uploads` (allocate) | 1 | 1 HEAD probe (`check_reachable`) | 1 SELECT + 1 INSERT | 1 |
| `POST .../complete` | 1 | HEAD staging, COPY, HEAD final, range GET | 4–5 SELECT/UPDATE | 4 |
| `GET /api/media/{id}/file` (image) | 1 | 1 GET (via 307) | 4 SELECT | 0 (presign only) |
| `GET .../info` + `/file` (audio/video) | 2 | 1+ GET per load/range | 6 SELECT | 0 |
| `POST .../abandon` | 1 | 0 | 1 UPDATE | 0 |
| cleanup, per media | 0 | 2–3 DELETE | 1 UPDATE (+1 per batch scan) | 2–3 |

The server work itself (JSON parse, HMAC presign, auth) is microseconds; the
cost is round trips, the single SQLite mutex, and the two global semaphores.
At the default 64-user scale the system is comfortable; the problem is the
tail: a slow or unavailable bucket, or a large media table.

## 1. Concurrency: the shared `io_jobs` permit (P0)

`App::io_permit` is a non-blocking `try_acquire_owned` on a 64-permit semaphore
shared by every chat endpoint (`server/src/web/mod.rs:53`, capacity set at
`server/src/main.rs:67`). It is the back-pressure mechanism for DB/Argon2 work,
and normal handlers hold it for the duration of a `spawn_blocking` call
(milliseconds). Media handlers hold it across external awaits:

- `allocate` holds it across `spawn_blocking` + `check_reachable` (up to 3 s,
  `server/src/web/media.rs:59`);
- `complete` holds it across `storage.gate.lock().await` (unbounded queue) and
  the four-call `finalize` (30 s per request timeout, `server/src/web/media.rs:123`);
- `info`, `download`, `abandon` hold it only around DB work, which is fine.

Failure mode: 64 concurrent completes (a batch upload from a few users, or
clients retrying a slow completion) occupy all permits while waiting on the
gate/S3. Every other endpoint then fails immediately with
`503 "Server busy; retry shortly."` — logins, history, message sends, bot
calls. If cleanup holds the gate while S3 hangs, it can hold it for
`32 media × 3 keys × 30 s ≈ 48 min`, so a bucket outage becomes a 48-minute
API outage. Client retries (120 s complete timeout) add more queued completes,
which is a positive feedback loop.

Fix:

1. Take the permit only inside the `spawn_blocking` closure (move it in, as
   `bots/socket.rs:62` already does), never across an S3 `.await`.
2. Add a dedicated `media_jobs` semaphore (4–8 permits) for outbound S3 work so
   a burst cannot open hundreds of sockets; queuing there is local and does not
   affect API permits.
3. Order: per-media lock → S3 + DB (permit only around DB) → response.

This is an availability fix; it does not make the happy path faster, but it
removes the only way media load can take down unrelated endpoints.

## 2. Serialization: one global gate (P0)

`storage.gate` (`tokio::sync::Mutex<()>`) is taken by `complete`
(`server/src/web/media.rs:125`) and by `cleanup` (`:238`) but not by
allocation or views. It exists to prevent this race:

- cleanup claims a row (`deleting=1`), deletes `uploads/{id}` and the final key;
- a late `complete` copies staging → final after the delete, recreating an
  object whose row is then removed → orphan object with no DB record.

Consequences today:

- completion throughput is `1 / (4 RTT + DB)`; at 100 ms RTT ≈ 2/s, so 64 users
  each retrying a completion queue for tens of seconds;
- cleanup holds the gate for its entire batch, including up to 3 S3 deletes per
  media with 30 s timeouts.

Fix: a per-media lock (`Mutex<HashMap<String, Arc<Mutex<()>>>>` with entries
removed when the row settles, or a small `DashMap`). `complete` and cleanup for
the same id serialize; different ids proceed in parallel. The DB claim
(`expired_media` setting `deleting=1`) remains the source of truth; the lock
only orders object deletion vs. copy so the orphan case cannot happen. Then
cleanup can process its batch with bounded concurrency (e.g. 4–8 in flight).

## 3. S3 round trips

### 3.1 `check_reachable` probe on every allocation

`allocate` makes a HEAD to `uploads/.availability` before inserting the
allocation (`server/src/media.rs:98`). That is one extra RTT per upload (and up
to 3 s on failure, while holding the permit — see §1).

Options, in order of value:

- cache a successful probe for 5–30 s in the `MediaStorage` (an
  `AtomicU64`/`Mutex<Option<Instant>>`); a burst of 8 pending uploads then pays
  at most one probe;
- drop the probe when the previous upload by the same user completed within the
  cache window (same thing, simpler key);
- keep the probe only on the first allocation after startup and after an S3
  error (fast-fail still works: a cached "up" state plus a failed PUT/complete
  surfaces the outage on the next request anyway).

The docs promise "no reservation when storage cannot be reached"; a cached
probe still honours that within the cache window, and the completion path
remains authoritative (HEAD staging fails → 400).

### 3.2 `finalize` is four sequential calls

`HEAD staging` → `COPY` → `HEAD destination` → `GET range 0..4095`
(`server/src/media.rs:124-165`). The final HEAD is redundant: the range GET
already returns `Content-Range: bytes 0-4095/<total>`, which proves the
destination exists and gives its size; the same response classifies the
content type. Dropping the final HEAD makes it three calls and preserves the
security property (classification happens on the copied bytes, not on the
staging object).

Further: the staging HEAD and the post-copy GET cannot run in parallel, but the
HEAD can be replaced by the range GET on staging (it returns the total size
too), then COPY, then the range GET on the destination: still three calls, but
only one kind of request. Keep the HEAD if you prefer the explicit size check;
either way, saving one RTT per completion is ~10 ms same-region, ~100 ms
cross-region.

### 3.3 Cleanup issues per-media deletes

For each expired media, `cleanup` deletes `uploads/{id}`, `final_key()`,
legacy `media/{id}`, and `object_key` if distinct
(`server/src/web/media.rs:243-276`). Every v14 row issues the legacy key
delete even though v14 objects never live there, and `object_key == final_key`
for rows written by `finish_media`, so a normal cleanup is two DELETEs plus one
guaranteed 404. At batch 32 that is ~96 requests per run.

- Skip the legacy delete when `media.object_key` is non-empty and starts with
  `media/` (v14), or store a `legacy` bit at allocation.
- Batch: rust-s3 0.38 provides
  `delete_objects<I: Into<Vec<ObjectIdentifier>>>`
  (`rust-s3-0.38.0/src/bucket.rs:2704`). One S3 DeleteObjects call covers up to
  1000 keys, including the staging keys; handle per-key errors in the response
  and mark `remove_media_record`/`staging_clean` per success. Verify the
  target provider (RustFS, R2) supports DeleteObjects before relying on it.
- `delete` already treats 404 as success, which makes idempotent retries cheap.

### 3.4 Duplicate validation (micro)

`allocate` validates in the handler (`validate_media_allocation`) and again
inside `allocate_media` (`server/src/engine/media.rs:194`); `complete` calls
`media_owner` before `finalize` and `finish_media` calls it again
(`server/src/engine/media.rs:255`). These are in-memory checks plus one JSON
parse each, so the cost is small — but `finish_media` can take the checked
`Media` as an argument instead of re-reading it. The outer check should stay
(it prevents an S3 probe for a request that cannot allocate), but the two
checks should share a resolved `Scope`: resolve once in the handler, and
re-check only what can change across the await (account, scope access, quota)
inside `allocate_media`. Do the same in
`download`: one `readable_media` before signing and a cheap re-check
(`media_attached` + expiry) after signing instead of a second full read; the
authorization window that matters is the message-reference check, not the JSON
decode.

## 4. HTTP caching and playback fan-out

Every response gets `Cache-Control: no-store` from the security middleware
(`server/src/web/security.rs:54`), and the presigned S3 GET carries
`response-cache-control: private, no-store` (`server/src/media.rs:190`).
Presigned URLs also include a fresh signature and date on every call, so even
if the browser cached the bytes, the next `/file` request would have a
different URL — the only way to reuse bytes is to cache the 307 itself.

For images this is the largest avoidable read amplification: rendering a room
with 20 images on each re-render (or each navigation) issues 20 chat requests,
each doing 2 auth checks + 4 SQLite reads + a presign, then 20 S3 GETs. With a
short-cacheable redirect:

- `/file` sends `307` + `Cache-Control: private, max-age=30` (only when the
  media is inline; keep `no-store` for `?download=true` and for HTML/SVG
  previews);
- the S3 GET uses `response-cache-control: private, max-age=60` (bounded by
  the URL lifetime, which is already `min(remaining, 60)`);
- repeat views within the window go browser → S3 directly; the chat server sees
  one request per 30 s per object instead of one per view.

Implementation note: the middleware inserts `no-store` unconditionally, so a
handler-set header is overwritten. Change it to insert only when the response
has no `Cache-Control`, or expose a small helper that sets the policy after the
middleware.

Trade-off: a revoked/expired/retracted media remains fetchable for up to 30 s
(the URL was already minted with a 60 s signature, so this is consistent with
the existing model). Document the window in `docs/security.md` next to the
revocation semantics; keep it configurable if that matters.

For audio/video, range requests are the amplification: each `<video>`/
`<audio>` range request that hits `/file` is an auth + 4 SQLite reads + presign
+ 307; with `preload="none"` the initial load is cheap, but seeking can issue
many ranges. The cacheable redirect fixes the burst within its window. If
per-range chat load still shows up, `/info` can return a 60 s signed
`playback_url` and the player can use it directly for that element (the URL is
already exposed in `Location` today, so this is not a new leak; it trades
per-range revalidation for URL lifetime). Do that only for audio/video, not
for documents.

HTML/SVG previews are click-driven and capped at 1 MiB, so they are not worth
caching; keep `no-store` there.

## 5. SQLite: plans, indexes, and counters

Measured with `EXPLAIN QUERY PLAN` on a scratch DB using the real schema
(`server/src/engine/storage/media.rs:5`):

| Query | Plan | Problem |
|-------|------|---------|
| `expired_media` SELECT | `SEARCH media USING INDEX media_expiry (deleted=?)` + `USE TEMP B-TREE FOR ORDER BY` | the `OR` over `deleting/expires/unreferenced` and `ORDER BY cleanup_attempt,created` sort all live rows; `LIMIT 32` does not stop early |
| blanket `UPDATE media SET unreferenced=now ...` | `SCAN media` + per-row `message_media_id` lookup | full table write scan every `cleanup_interval` |
| staging SELECT | `SCAN media` + temp B-tree | no index on `(staging_clean, created)` |
| allocation aggregate | `SEARCH media USING INDEX media_uploader (uploader=?)` + table lookups | not covering; O(all rows of that uploader) per allocation |
| `media_attached` | covering `message_media_id` lookup | fine |
| `reconcile` validity check | PK lookup + covering `message_media_id` | fine |

Recommended schema changes (SQLite partial indexes are supported):

```sql
CREATE INDEX media_due_expiry  ON media(expires)      WHERE deleted=0 AND deleting=0;
CREATE INDEX media_due_orphan  ON media(unreferenced) WHERE deleted=0 AND deleting=0 AND unreferenced IS NOT NULL;
CREATE INDEX media_due_deleting ON media(cleanup_attempt, created) WHERE deleted=0 AND deleting=1;
CREATE INDEX media_staging_due ON media(created)      WHERE deleted=0 AND staging_clean=0;
CREATE INDEX media_tombstone   ON media(created)      WHERE deleted=1 AND staging_clean=1;
CREATE INDEX media_pending     ON media(created)      WHERE deleted=0 AND unreferenced IS NULL;
```

Then split the `OR` query into a `UNION ALL` of three index-friendly branches
(deleting first for retry fairness, then expiry, then unattached-past-TTL) so
each branch can stop at the limit. Keep the per-row `deleting=1` +
`cleanup_attempt` update; the claim is what makes cleanup crash-safe.

Allocation: either extend the index to cover the aggregate, e.g.
`CREATE INDEX media_quota ON media(uploader, deleted, published, created, size)`
(SQLite can answer `SUM(size)`/counts index-only), or maintain
`media_usage(uploader, bytes, pending, window_start, window_count)` updated in
the same savepoint. The counter is O(1) and also makes `pending_limit`
enforcement independent of table size; it needs a migration/backfill and
careful decrement on `reconcile` (published), `abandon`, and cleanup. The
covering index is the cheaper first step.

Other DB items:

- `reconcile` runs `DELETE FROM message_media ...` for every message save, even
  text-only ones (`server/src/engine/storage/media.rs:13-51`). It is a no-op
  index delete on an existing transaction, but it can be skipped when neither
  the previous nor the new text contains `/api/media/`.
- `stored_media` + `media_attached` are two round trips; a single
  `SELECT json ... WHERE id=? AND EXISTS(...)` halves the read path. `download`
  currently runs both twice.
- A read-only connection pool (WAL is already enabled,
  `server/src/engine/storage.rs:57`) would let views read while a message
  commits; the current single `Mutex<Connection>` serializes them. Only worth
  it if measured view QPS approaches ~100/s; at 64 users the mutex is not the
  bottleneck.
- `synchronous` is already NORMAL under WAL (verified), so no fsync-per-commit
  tuning is needed. Add `PRAGMA busy_timeout=5000` if a read pool is added.

## 6. Cleanup throughput

`cleanup_interval=300`, `cleanup_batch=32` → at most 384 media per hour. With
the default 1 GiB quota and 1 MiB files that is 64k objects per 64 users → ~7
days to drain a full mass deletion; with 25 MiB files it is ~2.6k objects →
~7 h. Acceptable for normal expiry, too slow for an incident (account deletion,
room purge) where the operator expects space back quickly.

Also:

- `tokio::time::interval` uses `MissedTickBehavior::Burst`
  (`server/src/web/media.rs:217-221`): if a run takes longer than the interval,
  the next ticks fire immediately, so cleanup runs back-to-back and keeps the
  gate hot. Use `set_missed_tick_behavior(Skip)` and add ±10% jitter (also
  avoids synchronized runs if more than one process is ever deployed).
- Process batches until the batch is short or a time budget (e.g. 20 s) is
  exhausted, rather than exactly one batch per tick; then a backlog drains in
  one run without holding the gate continuously.
- `cleanup_interval`'s validation allows 1 s (`server/src/config.rs:72`);
  a misconfiguration would issue 32 deletes every second. A floor of 60 s is
  safer.
- Pending slots are freed only when cleanup removes the row: `abandon` sets
  `deleting=1` but `pending_limit` counts `deleted=0 AND published IS NULL`
  (`server/src/engine/storage/media.rs:54-77`). If S3 deletes keep failing, a
  user who abandons uploads can hit `pending_limit` and get "Upload quota
  reached" until cleanup recovers; the warning log is the only signal.
- Staging cleanup scans all rows because `staging_clean=0` has no index; the
  partial index above fixes it. Rows whose staging object is gone (crash) are
  handled by the accepted 404.

## 7. Upload ceiling: 5 GiB is not achievable

`MediaConfig::validate` allows `max_size` up to 5 GiB
(`server/src/config.rs:68`), but:

- a single presigned PUT is limited by S3 to 5 GB (5,000,000,000 bytes), and
  `CopyObject` is also limited to 5 GB (multipart copy is required above that);
- the presigned PUT expires after 300 s (`server/src/media.rs:120`), so a
  5 GiB object would need ~143 Mbit/s sustained; 25 MiB needs ~0.67 Mbit/s,
  which is already a real constraint on poor uplinks.

Consequences: configure `max_size` above 5,000,000,000 and uploads fail in
confusing ways (PUT 400/complete 400); configure a large size and clients on
slow links time out mid-PUT (the presigned URL dies at 300 s and the retry
cannot resume).

Options:

- cap validation at 5,000,000,000 (single PUT + copy) and document the 300 s /
  bandwidth math in `docs/media-uploads.md`;
- or implement multipart upload (presign per part + `CompleteMultipartUpload`
  + multipart copy) if large media is a goal; that is a larger change and
  needs abort-on-cleanup handling;
- in the client, the XHR timeout is 300000 ms (`web/src/api/media.ts`), equal to
  the URL lifetime, so a transfer that fits the URL always fits the XHR
  timeout; keep those two values derived from one config, and surface a clear
  "upload too large for this connection" message rather than a generic timeout.

## 8. Client-side

- `useMediaUploads` writes the upload percentage into its `status` string on
  every XHR `progress` event (`web/src/hooks/useMediaUploads.ts:100-102`). On a
  fast link that is hundreds of state updates and re-renders per upload.
  Throttle to animation frames or ~10 Hz, or write the bar width via a ref.
- The image preview creates an object URL for any `image/*` up to `max_size`
  (25 MiB default) and lets the browser decode it for a thumbnail. Cap the
  thumbnail at a few MiB (show an icon above that) to avoid large decodes and
  blob retention between complete and send.
- `completeMedia` timeout is 120 s, while the server can spend 4 × 30 s on S3
  in the worst case before the DB update. Align them: either cap the server's
  per-request S3 timeout at ~10 s (and return 503 on timeout) or raise the
  client to ~150 s. Today the client can abort exactly as the server commits,
  then retry (idempotent, cheap), but the retry is avoidable noise.
- `info` + `file` for audio/video is two chat requests; see §4 for folding a
  short signed playback URL into `/info` if per-range load matters.
- `mediaMarkdown` re-encodes the absolute path (intentional). The `?playback`
  cache-buster in the player invalidates cached streams on retry — intentional,
  but note that it resets range-request reuse too.

## 9. Observability and benchmarks

There are no media-specific spans or counters today; `tower_http::trace`
(`server/src/web/mod.rs:135`) logs one line per HTTP request, which is not
enough to tell S3 time from lock time from DB time.

Add, behind the existing `tracing` setup:

- spans on `allocate`, `complete`, `download`, `cleanup` with fields
  `media_id`, `bytes`, `s3_calls`, `lock_wait_ms`, `s3_ms`, `db_ms`;
- counters/logs for `503 media_busy`, `cleanup_batch_full` (backlog > batch),
  `cleanup_errors`, and `probe_failures`;
- one log line per cleanup run: `expired`, `deleted`, `failed`, `elapsed_ms`.

Benchmarks (local RustFS, same machine):

1. baseline: 50 concurrent `GET /api/media/{id}/file` for one image, measure
   p50/p95/p99 and SQLite wait; repeat with the redirect cache to quantify §4;
2. 20 concurrent `complete` with a 1 s S3 latency injected by toxiproxy in
   front of RustFS; concurrently poll `/api/history` — today you should see the
   `503 Server busy` cascade; after §1/§2 it should stay 200 and completes
   should queue locally;
3. cleanup with 10k media rows: compare `EXPLAIN QUERY PLAN` and wall time
   before/after the partial indexes; assert `SEARCH ... USING INDEX` and no
   `TEMP B-TREE` for the due-set queries;
4. upload: 25 MiB over a shaped 1 Mbit/s link to confirm the 300 s window is
   the real limit (and that the client message is understandable).

## 10. What I would not do

- Proxy media bytes through the chat server: it moves S3 bandwidth into the
  single-process server, breaks range/seek efficiency, and defeats the signed
  URL design.
- Cache aggressively beyond the URL lifetime or for `?download=true`: signed
  URLs are already the security boundary; a long redirect cache is a
  revocation-lag decision, not a free win.
- Index `media_scope`'s linear scans: rooms are capped at 1024 and private
  chats at ~32k only if every user pairs with every other (they are created
  lazily); a scan over the `BTreeMap` values is tens of microseconds. Note that
  a `contains_key` shortcut is not equivalent: the private map is re-keyed on
  account rename while `chat.id` is not (`server/src/engine/accounts.rs:43-54`),
  and rooms are keyed by name while `room.id` is a UUID. Leave both scans.
- Split the S3 client or presign on a thread pool: presigning is local HMAC
  (microseconds); the cost is network RTTs.
- Enable gzip for previews or transcode images: explicitly out of scope in the
  design (signature-only classification, no decoding).

## 11. Correctness and robustness

These were raised in the first review. C1 and C2 are the two that also dominate
the efficiency picture, because they turn a slow dependency into an outage.

**C1 (HIGH) — the shared `io_jobs` permit is held across S3 awaits.** The media
handlers acquire `app.io_permit()` and keep it for the whole handler:
`allocate` holds it across `check_reachable` (`server/src/web/media.rs:59`), and
`complete` holds it while waiting for the gate and for four sequential S3 calls
(`:123`). `io_permit` is `try_acquire_owned` and fails closed with 503 "Server
busy; retry shortly." (`server/src/web/mod.rs:53-57`). The same 64 permits
serve logins, history, commands, and bots, so enough stalled media requests can
deny every other endpoint. The fix in §1 (permit only around DB work, separate
small S3 semaphore) also removes this failure mode.

**C2 (HIGH) — one global `storage.gate` serializes completions and cleanup.**
`complete` takes `storage.gate.lock().await` before `finalize`
(`server/src/web/media.rs:125`), and `cleanup` holds the same lock for a whole
sweep (`:238`). The sweep performs up to `cleanup_batch` × (staging, final, and
legacy deletes, 30 s timeout each) sequentially; a slow bucket can hold the gate
for up to ~48 minutes (32 × 3 × 30 s). Completions then queue *while holding
permits*, which converts C2 into C1. The fix in §2 (per-media claim/lock, DB-side claim, bounded
drain) addresses both.

**C3 (MEDIUM) — `complete` reports storage failures as HTTP 400.** `finalize`
errors are mapped through `failed()` at `server/src/web/media.rs:138`, but
`finalize` mixes genuine client errors (staging size mismatch, missing upload)
with transient storage errors (copy/HEAD/GET failures, timeouts, credential or
endpoint problems). Clients cannot tell "your file is wrong" from "try again",
and a 4xx does not trigger the retry/alerting paths a 5xx would. Suggested fix:
return a typed error (`Invalid` | `Unavailable`) from the storage layer and map
to 400/503; keep the underlying S3 cause in a server log keyed by media id.
Note that `check_reachable` and the presign calls already return 503, so
`complete` is the inconsistent path.

**C4 (LOW) — zero-size underflow in `finalize`.** `get_object_range(...,
Some(media.size.min(4096) - 1))` at `server/src/media.rs:154` underflows when
`size == 0`. `allocate_media` rejects size 0, so this is only reachable through
corrupt or hand-edited data, but `saturating_sub(1)` plus an explicit
`size == 0` error makes the transport layer self-defending.

**C5 (LOW) — deployment prerequisites are easy to miss.** `setup_local_s3`
only allows `http://127.0.0.1` and registers CORS for ports 5173
(`server/src/media.rs:223-253`), so a dev server on another port fails at the
PUT with an opaque browser CORS error. In production the nginx CSP must have
the storage origin added to `connect-src` and `media-src` by hand
(`deploy/nginx.conf:51-63`), and a signed GET is served by the bucket, so the
application cannot add `X-Content-Type-Options: nosniff`; the bucket must do
it. Both are documented, but both are the kind of step that succeeds in staging
and fails in production.

**C6 (NIT) — allowance edge cases.** `su` gets the implicit `config.max_age`,
not "permanent", so uploading `-1` media needs an explicit `-1` grant. A media
grant with omitted limits stores `None`, so raising `media.max_size` or
`media.max_age` later also raises what every such grant allows; if the intent
is "up to what I held when I granted", snapshot the numbers at grant time.
Relatedly, the implicit private allowance requires the user to have at least
one group (`server/src/engine/media.rs:103-106`); account creation always
assigns one, so this only matters if a group can be revoked down to zero while
the account still participates in a private chat.

**C7 (NIT) — zero-row `abandon` is silent.** `abandon_media` is idempotent and
returns `Ok` when the row was already published or deleted
(`server/src/engine/storage/media.rs:121-131`). That is a reasonable API, but
without a log or counter a double-abandon or a late abandon is invisible; a
debug log costs nothing.

## 12. Code quality and redundancy

The feature is generally well factored (authorization and storage are separate,
validation is server-side, the client re-verifies everything it renders), but
the upload path accumulated a layer of repeated checks and hand-mirrored state.

### 12.1 Duplicate checks and dead work

**Q1 — dead command check.** In the media branch of `/grant`/`/revoke`,
`require_target_command(actor, &scope, parts[0])` runs at
`server/src/engine/authorization/commands.rs:250` and then again at `:259`,
with only a user lookup between them. The second call cannot fail unless the
first did. (The same function also validates the part count `(4..=6)` up front
and then re-checks `parts.len() != 4` for `/revoke` at `:373`; one validation
pass would be clearer.)

**Q2 — the same policy is evaluated twice per request.** `allocate` calls
`validate_media_allocation` at `server/src/web/media.rs:71` and `allocate_media`
calls it again at `server/src/engine/media.rs:201`; `complete` calls
`media_owner` at `server/src/web/media.rs:132` and `finish_media` calls it
again at `server/src/engine/media.rs:256`. Some repetition is intentional: the
pre-check runs before the S3 probe so a rejected upload never reaches the
bucket, and re-checking after an `await` closes a TOCTOU window. But the
engine API can express that directly: have the pre-check return the resolved
`Scope` (and the `Media` for `complete`) so the second call validates against
the same snapshot instead of re-deriving it, and keep a single re-check across
each await boundary.

**Q3 — duplicated response construction.** `complete` and `info` build the
same five-field object, including the
`format!("{}api/media/{}/file", app.config.base_url, ...)` path, at
`server/src/web/media.rs:152` and `:171`. One `media_json(&media)` helper
prevents the two paths from drifting (for example, if the path shape changes).

**Q5 — the conversation is resolved twice.** `media_scope` linearly scans
`rooms` and `private` (`server/src/engine/media.rs:153-170`); callers then
re-derive the scope from the id via `media_conversation_scope`
(`:180-183`, `:241-246`). Returning `Scope` from `media_scope` removes a second
scan per call. (This is also why §10 leaves the scans alone: with the default
64 rooms the scan is cheaper than maintaining a second index.)

**Q6 — two locks and two queries on the read path.** `media_owner` and
`readable_media` both do `stored_media(id)` and then `media_attached(id)`
(`server/src/engine/media.rs:246-248`, `:264-271`), which takes the engine
mutex twice and runs two statements. A single
`SELECT json, EXISTS(SELECT 1 FROM message_media WHERE media_id=?)` returns both,
and `readable_media` can reuse the row it already read.

**Q7 — `reconcile` runs for every message, not just media messages.**
`put_message` calls `reconcile` unconditionally, and `reconcile` always
executes `DELETE FROM message_media WHERE conversation=?1 AND sequence=?2` and
re-inserts every referenced id (`server/src/engine/storage/media.rs:34-50`).
For the common text-only message that is one write statement per send. An
early return when `media_references(text)` is empty and the old reference set is
also empty (one `EXISTS` lookup or a cached count) would make plain chat writes
as cheap as they were before the feature. Since `reconcile` is inside the
message savepoint, this is pure overhead, not a correctness requirement.

**Q8 — handler boilerplate.** The six handlers in `server/src/web/media.rs`
(`config:32`, `allocate:51`, `complete:115`, `info:157`, `download:180`,
`abandon:281`) each repeat the origin/bot check, permit acquisition,
`spawn_blocking` authenticate, and error mapping; five of them also repeat the
`storage()` lookup. Extractors
(`MediaStorage`, `MediaUser`) would remove roughly 60–80 lines and, more
importantly, make the permit scope explicit at the call site instead of relying
on a `_permit` binding at the top of the function. `config` is a pure
memory/auth response and arguably should not consume an I/O permit at all.

**Q11 — silent or generic failures.** `abandon_media` returns `Ok(())` when the
`UPDATE` changes zero rows (`server/src/engine/storage/media.rs:121-131`), and
`store_media_allocation` returns the same "Upload quota reached" message for
the byte quota, the pending-slot limit, and the rate limit (`:60-66`). The
latter matters operationally: an operator cannot tell which knob to raise, and
a client cannot tell whether to wait or delete something.

### 12.2 Duplicated SQL and dual state

**Q4 — copy-pasted tombstone SQL.** The same
`DELETE FROM media WHERE deleted=1 AND staging_clean=1 AND created<?1 AND NOT
EXISTS(...)` appears at `server/src/engine/storage/media.rs:139`
(`expired_media`) and `:192` (`remove_media_record`). Extract a
`delete_old_tombstones(tx)` helper. While there, bind `now()` once per sweep:
`expired_media` calls it for the unreferenced update, the tombstone cutoff, the
selection parameters, and each per-row update, so a sweep that crosses a second
boundary can use inconsistent cutoffs (harmless today, surprising later).

**Q9 — object state is stored twice.** A media row has columns `ready`,
`expires`, `deleting`, `deleted`, `cleanup_attempt` and a JSON blob mirroring
all of them except `cleanup_attempt`; every mutation updates both by hand at
four sites
(`server/src/engine/storage/media.rs:87` `store_media_ready`, `:121`
`abandon_media`, `:139` `expired_media`, `:187` `remove_media_record`), while
`stored_media` reads the JSON and the SQL filters read the columns. The two can
diverge silently if a new field is added to only one path. Either make the
columns the single source of truth (JSON only for the immutable payload:
filename, size, content type, max age) or funnel every update through one
`write_media_state` helper that mirrors all fields. The former is the smaller
long-term surface.

### 12.3 Errors and observability

**Q10 — errors lose the cause.** `MediaStorage` returns
`Result<_, String>`; `finalize` and `delete` (`server/src/media.rs:124-219`)
stringify (or flatten) the `S3Error`, so a credentials problem, a 404, and a
timeout can look identical to the operator. Presign failures are dropped
entirely: `Cannot sign upload URL.` (`server/src/web/media.rs:106-109`) and
`Cannot sign download URL.` (`:202-205`) log nothing, which makes the one class
of failure that cannot be reproduced locally invisible. A small error enum
(`Invalid`, `Unavailable { source }`, `NotFound`) plus `tracing::warn!` on the
presign and finalize paths would pay for itself.

### 12.4 Types and naming

**Q13 — readability.** `media_scope(user, id, write: bool)`
(`server/src/engine/media.rs:153`) hides two different permission sets behind a
boolean; two functions (`media_scope_read` / `media_scope_write`) or an enum
reads better. `-1` as the permanent sentinel is used in three places (grant
constraint, `Media::max_age`, expiry math) and is easy to miss; a
`Retention::Permanent | Retention::Seconds(i64)` type would make the branches
explicit. `media_allowance` (`:79-124`) is a long boolean expression with
`private_default` bound twice; splitting "collect candidates" and "filter by
required age" would make the su/room/private defaults auditable line by line.
`timestamp()` (`:25-27`) is a wrapper around `now()` that adds nothing.

**Q12 — client types and duplication.** `isUploadTicket` validates
`expires_in` (`web/src/api/media.ts:7,33`) but no caller reads it; either
surface the remaining lifetime in the UI or stop validating an unused field.
`Attachment` takes `kind: string` (`web/src/components/Attachment.tsx:10`)
although `AttachmentKind` exists, so an invalid kind is only caught at runtime.
`mediaMarkdown` can throw `Invalid attachment origin.` (`web/src/api/media.ts:65`)
inside `useCommands.send`, where an exception bypasses the normal error path;
return `null` and let the caller show a message. Finally, the media-path shape
is parsed twice: `isCompletedMedia` uses the anchored regex at
`web/src/api/media.ts:47-60`, and `attachmentLink` re-implements the same
`api/media/<36>/file` check at `:137-160`; one parser returning
`{id, kind, path}` keeps the two in sync.

### 12.5 Smaller cleanups

- `input.clone()` in `allocate` (`server/src/web/media.rs:63`) clones the whole
  request only to keep `filename`/`size` past the first closure; moving those
  two fields into the closure and passing them back avoids the clone.
- `web/src/components/Composer.tsx` expresses "attachment not allowed" on the
  Attach input (`:204-209`) and again on Send (`:251-256`); one derived
  `canAttach` value would keep the input and the button in sync.

### 12.6 Tests worth adding

Q14: unit tests for `download_url`'s `Content-Disposition` filename encoding
(space, quote, non-ASCII), `attachmentLink` with a non-root `base` path and
with `?playback=`, `abandon_media` on a published row, `reconcile` on a
media-free message, and a handler-level test that a storage timeout maps to 503
while a size mismatch maps to 400. These are cheap and pin the behaviors the
refactors above are most likely to disturb.

### What is already solid

Worth keeping: authorization is checked before the S3 probe and re-checked
after each `await`; download signs only after `readable_media` and re-checks
before redirecting; the engine owns the policy while storage owns the SQL;
presigning uses the public bucket so the browser never needs credentials;
objects are copied to a server-only key; the client validates ticket shape,
same-origin paths, content type, and size before rendering; HTML/SVG preview is
bounded, UTF-8-strict, and sandboxed. The issues above are cleanups around a
sound design, not a redesign.

## Suggested order of work

1. §1 permit scoping + §2 per-media lock (small, removes the outage mode).
2. §3.2 finalize call reduction + §3.1 probe cache (small, per-upload latency).
3. §4 redirect/byte caching + §7 size cap (small, biggest steady-state win).
4. §5 partial indexes + §6 cleanup loop (medium, keeps the tail flat).
5. Q4/Q7/Q9 storage helpers (small, follows the index work in the same file).
6. Q2/Q5/Q6/Q8 engine/handler cleanups (small–medium, after the P0/P1 changes
   settle so the refactor is not fighting the fix).
7. §8 client polish + §9 instrumentation (ongoing).
