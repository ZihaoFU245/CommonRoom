# Static review — media uploads, per-user metadata, command rework

Base: `4d385d114f6541f2d6917c89d93296c3d25d5732` (merge of #27) → `2011e87` (`Refactor scoped metadata, cleanup limits, and user resets`).
Scope: 88 files, +6733/−3279. Method: static reading only (no test execution; the suite already passes). All paths below are relative to the repository root.

## Verdict

The three features are coherent and the layering is mostly right: one command catalogue with a routing shim, a two-level (`@global` + room-id) metadata tree resolved at read time, and a media pipeline that separates SQLite reservations from S3 bytes. The hot paths are also better than the pre-rework code — a read-command fast path, per-message savepoints, and a bounded in-memory working set instead of cloning history.

The main risks are not in the happy path but at the **authorization boundaries the rework deliberately introduced**: room-scope readers now have a projection into global account metadata, `/grant` can mutate membership outside the invite check, and media reference validation has been split between the engine (cheap) and SQL (authoritative), which produces a late, differently-worded failure. There is also a systematic performance cost: every non-message mutation clones the whole engine state, including cached message deques, and immediately deep-compares it — contrary to what the architecture doc claims.

| # | Severity | Finding |
|---|---|---|
| 1 | High | `metadata_section` leaks global media limits, quota and role-derived cleanup limit to room readers (`global_read` guards only custom root keys) |
| 2 | High | `/grant <user> <group> @global` mutates then fails with “Room not found.” |
| 3 | Medium | Group `/grant` inserts room membership without checking `x:member.add` |
| 4 | Medium | Media ownership/conversation checks are deferred to `reconcile`, after the message write, with a different error text |
| 5 | Medium | `media-uploads.md` CORS sample omits the hint headers, so documented self-hosting breaks the PUT preflight |
| 6 | Medium | Administrative commands clone the whole engine state (including cached message deques) and the generic write path deep-compares it twice |
| 7 | Medium | `/clean` counting is N+1 per conversation, unindexed `json_extract`, under the engine read lock |
| 8 | Low | A failed media allocation still persists an empty private chat |
| 9 | Low | Service worker: `/info` round-trip per request, non-LRU eviction, range entries counted against the same caps |
| 10 | Low | Client hint truncation can split a `%XX` escape |
| 11 | Low | Bulk metadata set/reset mutates before it can fail; atomicity relies on the dispatch rollback |
| 12 | Low | `media.user_quota` special-casing is spread across eight sites and yields surprising bulk selection |
| 13 | Low | `/clean` commits and bumps the revision even when nothing matched |
| 14 | Low | Docs/code drift: rollback-by-clone, `@global` group grants, CORS, restart semantics |
| 15 | Nit | Duplicated hint validation (three calls), manual PUT header rebuild, linear `scope_label`, absolute media URLs in message text |

---

## 1. High — room readers can read global account metadata

`/user get` always builds an `@global` section and only passes `global_read` as a hint:

`server/src/engine/objects.rs:173-189`
```rust
// Room authority must not reveal arbitrary global account metadata.
let global_read = self
    .metadata_access(context.actor, &name, &Scope::Server, false)
    .is_ok();
let mut sections = BTreeMap::new();
sections.insert(
    "@global".to_owned(),
    self.metadata_section(&name, &Scope::Server, key, global_read)?,
);
```

`metadata_section` uses `global_read` for exactly one thing — the loop over custom root keys:

`server/src/engine/objects.rs:245-260` (guarded)
```rust
if global_read && let Some(root) = metadata.get("@global") {
    for (key, value) in root {
        if scope != &Scope::Server && key == "media.user_quota" { continue; }
        ...
```

Everything after that is unconditional (`server/src/engine/objects.rs:274-315`):

```rust
for (key, default) in [
    ("media.max_size", self.media_config.max_size.to_string()),
    ("media.max_age", self.media_config.max_age.to_string()),
    ("media.user_quota", self.media_config.user_quota.to_string()),
] {
    if key == "media.user_quota" && scope != &Scope::Server { continue; }
    let local = metadata.get(Self::metadata_scope_key(scope)?).and_then(|m| m.get(key));
    let global = metadata.get("@global").and_then(|m| m.get(key));
    ...
}
let (limit, source) = self.cleanup_limit(name, scope)?;
```

When the section is the `@global` section, `scope == Scope::Server`, so:

* `media.max_size` / `media.max_age` come from `metadata.get("@global")` and are labelled `"@global"`;
* the `media.user_quota` skip condition is false, so the target's **global quota** is displayed;
* `cleanup_limit(name, &Scope::Server)` falls back to live roles, so `history.clean_min_age` reveals whether the target is a global admin/su (`-1`) or an ordinary user (`0`).

Who can reach it: any actor with `x:policy.change` at a room (room owner/admin) but *without* `r:account.metadata`, where the target is a member of that room (`metadata_targets` enforces membership at `objects.rs:69-76`). `metadata_access` gates on `PolicyWrite` for a room scope and on `AccountMetadataRead` for the account scope, so this is exactly the actor the comment intends to stop. `/user get bob #lobby` as the room owner therefore returns `bob`'s global media policy and role-derived cleanup limit.

Impact: policy leakage (quota/size/retention limits are admin-set per-account values) and role disclosure. No stored secret leaks — values are clamped to the server ceiling — but the documented contract is “filtered by account/room read authority” (`docs/architecture.md:213`), and `docs/commands.md:41-44` says `x:account.metadata` is reserved to su.

Fix options: (a) drop the `@global` section from `query_objects` when `global_read` is false, (b) pass a “local-only” flag into `metadata_section` that makes `global` lookups miss and skips `cleanup_limit`'s role fallback, or (c) compute `cleanup_limit` from the room branch only for room sections. Option (a) is the smallest and matches the comment.

Test gap: `server/src/engine/objects.rs:1128` asserts that *eve* (no policy write) is denied, and `:1089` is a self-read where `global_read` is true. No test exercises the room-manager-reads-another-account path, which is the only path where the flag is false.

## 2. High — `/grant <user> <group> @global` mutates, then errors

`server/src/engine/authorization/commands.rs:479-490` (group branch of `apply_policy`):

```rust
self.assign(target, group, scope.clone());
if let Some(name) = parts.get(3) {
    self.data.rooms
        .get_mut(name.trim_start_matches('#'))
        .ok_or("Room not found.")?
        .members
        .insert(target.to_owned());
}
```

`/grant bob admin @global` parses `parts = ["/grant","bob","admin","@global"]`; `object_scope` accepts `@global` and returns `Scope::Server`; `assign` pushes the assignment; then the code looks up a **room named `global`**. Result: `Err("Room not found.")`, and the caller only sees a clean state because `execute_inner` restores `before` (`dispatch.rs:301-305`).

Consequences:

* A legal, natural spelling of a global group grant fails with a misleading error. `/grant bob admin` (scope omitted) works, and `/revoke bob admin @global` works, so grant/revoke are asymmetric.
* `/grant @all admin @global` fails the same way through the recursion.
* The failure happens *after* mutation, so any future caller without the whole-state rollback (a bot path, a background job, a new API) would persist a grant with no membership and report failure.

`server/src/manual.rs:237` documents the form as `/grant username [user|admin|su] [#room]`, and `docs/commands.md` says “Without a room, the group applies at global scope” — so `@global` is implied but not written, which is precisely why the bug lived: no test and no example use it.

Fix: gate the membership insert on `matches!(scope, Scope::Room(_))` and resolve the room from the scope id, not the raw token; then add a test for `/grant bob admin @global` and `/grant @all admin @global`.

## 3. Medium — `/grant` adds room membership without the invite action

Same block. `require_member_delegation(actor, scope)` checks that the actor holds every *member* right at the scope, but the insert itself is the equivalent of `/room add`, which requires `Action::Invite` (`server/src/engine/rooms.rs` `/add` arm: `require(actor,&scope,Action::Invite)` plus `require_member_delegation`).

`Action::Invite` lives in `manager_rights`, not in `member_rights`, so an actor holding `x:policy.change` plus all member actions but not `x:member.add` can still add an account to the room by running `/grant <user> user #room`. Room admins all have both, so this is not reachable through the default groups — only through a custom direct grant, which is exactly the case the delegation ceiling is supposed to cover.

Fix: require `Action::Invite` (or reuse the `/add` helper) before inserting, and audit it as a membership change rather than as a grant.

## 4. Medium — media references are validated in the wrong layer, with a different error

`server/src/engine/media.rs:121-133` `authorize_media_text`:

```rust
let previous = media_references(previous);
for id in media_references(text).difference(&previous) {
    let media = self.stored_media(id)?;                                  // any stored row, any owner, any state
    let user = actor.ok_or("Media uploads require an account.")?;
    self.require(Some(user), scope, Action::Read)?;
    self.require(Some(user), scope, Action::Send)?;
    self.media_allowance(user, scope, media.size, Some(media.max_age))?;
}
```

(`stored_media` is `stored_media_with_reference` minus the flag; its missing-row error is `"Upload not found."` at `server/src/engine/storage/media.rs:152`. Note also that ids already present in `previous` are skipped entirely, which is deliberate for expiry preservation but means an edit that keeps a reference never re-validates it.)

The authoritative check is the inline `SELECT EXISTS(...)` in `reconcile` (`server/src/engine/storage/media.rs:66`), run per new id before the references are replaced:

```sql
... WHERE id=?1 AND conversation=?2 AND ready=1 AND deleting=0 AND deleted=0
      AND (expires IS NULL OR expires>?4)
      AND (uploader=?3 OR EXISTS(SELECT 1 FROM message_media WHERE media_id=?1 AND conversation=?2))
```

which fails with `"Attachment is unavailable or belongs to another conversation/user."`.

Problems:

* **Existence oracle**: `"Upload not found."` (never allocated) vs `"Attachment is unavailable or belongs to another conversation/user."` (allocated, but foreign/pending/expired) tells an attacker which random UUIDs exist.
* **Late failure**: the reference is rejected during the storage transaction, after the message change was prepared, so the user gets a confusing error and unclear retry advice.
* **Ordering leak in `/edit`**: `apply_messages` calls `authorize_media_text` *before* the `original.author_id != actor && !allows(EditAny)` check, so a non-author can distinguish “bad media id” from “not your message”. Cheap to reorder.

Fix: do the four conditions (conversation, `ready`, expiry, uploader-or-already-referenced-in-conversation) in `authorize_media_text` using a single `stored_media_with_reference` call, and keep the SQL check as a defensive constraint only (map it to the same message). Then move the `/edit` ownership check before media validation.

## 5. Medium — documented CORS sample is stale

`docs/media-uploads.md:262` still shows:

```json
"AllowedHeaders":["content-type"],
```

while `server/src/media.rs:441` configures:

```rust
Some(vec!["content-type".into(), "x-amz-meta-client-filename".into(), "x-amz-meta-client-content-type".into(), "x-amz-meta-client-label".into(), "x-amz-meta-client-client".into()])
```

Anyone following the manual bucket setup in a separate-origin deployment gets a preflight failure on the first upload that carries hints (the web client always sends `filename` and `client`), which reads as a broken product rather than a doc bug. The rustfs/bootstrap path (`auto/start-s3.sh`) sets it correctly, so only the documented manual path is affected.

## 6. Medium — whole-state clone + double deep compare per command

`server/src/engine/dispatch.rs:280-306`:

```rust
let before = self.data.clone();
let result = self.apply(actor, room, input, markdown);
match result {
    Ok(reply) => {
        if self.data == before { return Ok(reply); }
        ...
        self.save_with_removed_rooms(&removed_rooms, Some(&before))
    }
    Err(e) => { self.data = before; Err(e) }
}
```

`Data` contains every room and private chat, and each holds its cached `messages` deque, so the clone is O(users + rooms + grants + **all cached messages**). Then `self.data == before` walks all of it again, and `write_delta` compares each room a third time (`storage.rs:1018-1020`):

```rust
let changed = if kind == "room" {
    data.rooms.get(*key) != before.rooms.get(*key)
} else {
    data.private.get(*key) != before.private.get(*key)
};
```

Affected commands: everything routed through the generic write path — `/user set|reset`, `/grant|/revoke`, `/room new|delete|add|kick|owner|join|leave`, `/enable|/disable`, `/clean`, `/rm room`. `/rename room|user`, `/rm user` and the password flows early-return and clone inside their own handlers instead (`rooms.rs:24`, `accounts.rs:35`, `accounts.rs:294`), so they pay one clone but not the second compare; a fix has to cover both sites. On an instance with 50 rooms × 1000 cached messages this is tens of thousands of message clones and comparisons per metadata edit.

It also contradicts the docs: `docs/architecture.md:100` says “administrative rollback clones metadata rather than history” and `:123-124` says “successful commands do not clone all metadata/cursors”. Either the sentence should be corrected or the implementation should match it (snapshot only the maps that a family can touch — accounts/policy/rooms — and use a per-family dirty flag instead of `Data == Data`).

Mitigating context: the read fast path (`dispatch.rs:34-119`) is correct and covers the common web case, and message commands use per-conversation snapshots. The clone penalty is only for console/stdin (`server/src/console.rs` calls `execute`, not `read_command`) and for any `/sudo`-prefixed read.

## 7. Medium — `/clean` planning is N+1 and unindexed

`server/src/engine/cleanup.rs` builds one target per conversation and runs, per target, a single query with two correlated subqueries:

```sql
SELECT (SELECT count(*) ... json_extract(json,'$.time')<?2),
       (SELECT count(*) ... FROM message_media JOIN messages ...)
```

`json_extract(json,'$.time')` cannot use the existing indexes, so `/clean 90d @global` scans every message of every conversation serially while holding the engine read lock. On a large instance the preview becomes the slowest command in the system and blocks all other commands. Consider a `messages(time)` generated column + index, or one aggregate query grouped by conversation, and cap the number of targets per plan (or page the preview).

## 8–15. Lower-severity findings

**8. Failed allocation persists an empty DM.** `resolve_media_conversation` (`server/src/engine/media.rs:60-95`) saves the empty private chat before `validate_media_allocation` and before `allocate_media_with_hints` performs the quota/rate checks (`storage/media.rs:106-148`). A user who hits “Pending upload limit reached” or a size limit still gets a new, permanently empty conversation in both sidebars. `docs/media-uploads.md:90-92` acknowledges that unused pairs persist, but creating one on *rejection* is avoidable — validate size/allowance first, and defer the `save_empty_private` until the reservation is about to be committed. The same handler also calls `storage(&app)?` before `authenticated(...)` (`server/src/web/media.rs:84-85`), so unauthenticated callers can distinguish “media configured” from “media disabled” by status code; move the storage check inside the authenticated block.

**9. Service worker cache.** `web/src/features/conversation/mediaCache.ts`:
* `respond` fetches `/api/media/{id}/info` on every request, including cached ones (`mediaCache.ts:69-76`). Seeking in a video produces one auth+DB request per range; consider an `info` TTL keyed by media id, or accept the cost deliberately and document it.
* Eviction iterates `cache.keys()` (insertion order) rather than recency (`mediaCache.ts:191-206`), so re-watching a cached item never protects it, and a cold 200 MiB entry outlives a frequently used 2 MiB entry.
* Range responses are stored under keys that include `?range=`, so `bytes=0-`, `bytes=1000-`, `bytes=2000-` are separate entries counted against `entryLimit=128`/`byteLimit=256MiB`. A seek-heavy video can evict every other entry, including its own full copy.
* For a 200 response the rebuilt `Content-Length` comes from `/info` (`mediaCache.ts:163-167`), not from the object actually returned. If the metadata and bytes ever disagree the browser will truncate or stall the download; prefer the network response's `Content-Length` when present and fall back to `info.size`.

**10. Hint truncation.** `web/src/hooks/useMediaUploads.ts:93`:
```ts
filename: encodeURIComponent(file.name).slice(0, 256),
```
`encodeURIComponent` expands non-ASCII by 3× per byte, so a long name can be cut in the middle of a `%XX` escape. The server only requires printable ASCII, so it stores a value that is not valid percent-encoding. Hints are display metadata (never trusted), so this is cosmetic — but `slice` should be applied before encoding or trimmed to the last complete escape.

**11. Bulk mutations are not atomic by construction.** `apply_objects` validates access for every target first, but the per-user 32-key cap and `@unset` removal happen inside the mutation loop (`server/src/engine/objects.rs:320-405`), so a mid-loop failure is only undone by the dispatch rollback. That makes correctness depend on the caller, which is exactly the fragility item 6 is about. Collect validated `(user, key, value)` tuples first, then mutate, then save once.

**12. `media.user_quota` special-casing.** The root-only quota is enforced in the CLI parser (`objects.rs:336,340,386,392`), the projection (`:247,263,280`), validation (`:453`), the engine (`media.rs` `media_allowance`) and the storage layer (`storage/media.rs:116` reads `self.data.users[…]metadata["@global"]["media.user_quota"]` directly, bypassing the metadata resolution helper). One of those sites will drift. Also note the deliberate oddity at `objects.rs:340`:
```rust
let selection = if *who == "@all" && *key == "media.user_quota" && rest.is_empty() {
    self.object_scope(context.actor, context.view, None)?
} else { scope.clone() };
```
`/user set @all media.user_quota 1G` run from a room selects **that room's members** but writes their **global** value, so “all” means “all in the current view” for selection and “global” for the write. It is documented from the writer's perspective but is a trap for a code reader; an explicit `@global` requirement would be clearer.

**13. No-op `/clean` still writes.** `apply_cleanup` only queues targets with `count > 0`, but `save_history_maintenance` (`storage.rs:1127-1161`) always commits a transaction, writes the audit entry and bumps `self.revision`, so `/clean 90d #room` that matches nothing still makes every client refresh. Either skip the save when `pending_cleanups`/`pending_deletes` are empty, or drop the revision bump.

**14. Docs/code drift.**
* `docs/architecture.md:100,123-124` vs the clone in `dispatch.rs:280` (item 6).
* `docs/architecture.md:206` “history … survive renames” is correct: `store_account_rename` rewrites stored message authors (`rewrite_account_messages`, `storage.rs:350`) and re-keys private chats. But the in-memory cached deques are not rewritten (`accounts.rs:60-83` only touches `room.members`), so the session that performed the rename keeps showing the old author name until that message window is re-fetched from the store. Rewrite `messages[..].author` in the caches, or drop the caches on rename.
* `docs/commands.md:44` / `docs/security.md:275-276`: `x:account.metadata` is “reserved for su” — accurate (`metadata_access` checks `is_su` before `AccountMetadataRead`), but a direct policy grant of `x:account.metadata` is silently useless; the code already emits a scope hint for that case.
* `docs/media-uploads.md:202` “every tracked object without a retained message reference” correctly documents that a *completed but unsent* upload is deleted on restart (confirmed by `server/src/engine/media_tests.rs:520-575`). The web client has no way to tell that its upload became invalid: `complete` succeeds, then `send` fails with “Attachment is unavailable…”. A `GET /api/media/{id}/info` after reconnect, or a distinct “upload no longer available” error, would make the behaviour legible.

**15. Nits / maintainability.**
* `server/src/web/media.rs:126-140` validates hints a third time and rebuilds the PUT header map by hand (`upload_headers.insert("Content-Type", …)`) next to the signed `hint_headers`; let `hint_headers` return both the signed map and the client map from one place.
* `web/src/api/media.ts:87-105` embeds an **absolute** URL (`new URL(media.path, document.baseURI).href`) in the message text, so a deployment behind a different origin or a later domain move produces messages that point at the old origin; a root-relative path would be portable and is accepted by `media_references`.
* Hints are only consumed by S3 object metadata (the UI uses `media.filename` from the completion response), which is consistent with the “untrusted display metadata” comment at `server/src/media.rs:10-11`.
* Hint values are re-validated immediately before signing and a third time before the PUT headers are rebuilt; fold the client-facing map into `hint_headers` so there is one source of truth.
* `scope_label`/`room_scope` scan all rooms on every authorization error and audit line; cache id→name if profiles show up in traces.
* `start_cleanup` (`web/media.rs:243-272`) sleeps before checking `app.stopping`, so the detached task can hold an S3 batch for up to 20 s after shutdown begins; `tokio::select!` on a shutdown notify would exit promptly.

---

## Design review

### Media upload

The pipeline is well factored: **reserve row (`ready=0`) → presigned PUT with signed hints → `complete` → S3 copy to `final_key` with `REPLACE` metadata → publish**, with `unreferenced`/`published` timestamps distinguishing “never sent” from “published then orphaned”. Three things are notably good:

* Hints are treated as untrusted display metadata, validated at the edge, signed into both the PUT and the final `CopyObject`, and never used for authorization, content type, expiry or quota (`server/src/media.rs:10-35`, `:313`).
* The completion copy is deliberate: it prevents a reused upload URL from overwriting a published attachment, and the test at `server/src/media.rs:567-573` confirms tampered staging metadata is dropped.
* Deletion is two-phase (`deleting` → S3 delete → tombstone → `staging_clean`), which correctly handles a PUT that lands after an early abandon, and the web download endpoint re-authorizes before serving bytes.

Weak points, in order: the layered reference check (item 4), the allocation side effect (item 8), and the restart policy. That last one is a design decision rather than a bug — `prepare_media_cleanup` at startup marks **every** unreferenced row, so a user who completes an upload and then closes the tab during a deploy loses the attachment. The docs say so; the client experience does not. If unsent uploads matter, keep `ready=1 AND published IS NULL` rows to the pending grace period on restart and let the timer collect them.

The service-worker cache is a reasonable design (same-origin `/file` URLs only, `/info` auth check, `private, no-store` on the wire, byte and entry caps) but its eviction policy is not LRU and its key space is range-sensitive, so under video seeking it degrades to a near-zero hit rate while still filling the quota. If the goal is “reload without re-downloading a posted image”, cache only full responses (`range === null`) and touch entries on read (or maintain a small recency ring).

### Per-user metadata

The model is the right one: `@global` root plus sparse stable-room-id branches, explicit values only, resolution local → global → config default, and cleanup limits deliberately falling back to live roles so that role changes leave no stored privilege. Transactional pruning on room deletion and the `@unset`/`/user reset` symmetry are the right primitives, and the “only su writes, read authority is separate” split is clear.

Two design costs are worth naming:

1. **Projection authority is a two-dimensional problem** (`global_read` × section × key) and the code currently encodes it as a single boolean hint, which is what produced item 1. A cleaner shape is a `MetadataView { scope, can_read_global }` passed to the projection, with the built-in values derived from it rather than from the loop's own `scope` parameter.
2. **`media.user_quota` is a global value living in a scope-keyed tree**, which needs special cases at every layer (item 12) and a surprising bulk selector. Either promote quota to a first-class account field (it is not scoped, unlike the other two media keys) or make the write path require an explicit `@global`.

`history.clean_min_age` in the same tree is the opposite: it genuinely resolves per scope and its `-1/0/N` vocabulary is documented, validated, and tested. That part reads well.

### Command rework

The rework is an improvement: a single catalogue with `help_for` rendering, one `route` shim so the new `/user …`/`/room …` spellings reuse the old domain operations, family-level dispatch (`apply_objects`, `apply_accounts`, `apply_policy`, `apply_rooms`, `apply_messages`), a read fast path that avoids the write lock and the save, and per-conversation snapshots for message edits. The `/sudo` implementation (elevate in memory, run, rebuild from data on both success and error) is a nice way to keep privilege ephemeral.

Where it is weak:

* **Two parsers, one truth.** `validate_room_arguments` (raw tokens) and `route` (normalized, `#`-stripped) coexist with the domain parsers. `route` strips `#` from *every* `/room` tail token (`commands.rs:44-50`), which silently rewrites room-name arguments and makes `/room rename room #x` ambiguous (it renames a room literally called `room`). `validate_room_arguments` covers only a subset of forms: `/rm room x` is validated by no rule, while `/room delete x` is. Moving to a single parse into `(family, operation, args, scope)` would remove a class of these.
* **Rollback is whole-state or per-conversation**, with no middle. That is why item 6 and item 11 exist. A family-scoped `ChangeSet` would be both cheaper and more honest about what a command can touch.
* **Read vs write is decided twice** (the `read_command` allowlist and the `execute_inner` allowlist), and they are not the same list: `/user`/`/room` are missing from the execute-side read-only set, so console and `/sudo` reads take the clone path. Derive both from the catalogue (`read_only: bool` per command) instead of maintaining two matches.
* **`/reset`** is special-cased in `web/commands.rs` and `console.rs` before `execute`, so `dispatch`'s “console-only” arm is only a backstop. That is fine, but the two entry points duplicate the provision parsing; `provision_command` already normalizes it — call it from `dispatch` too and delete the backstop.

---

## Suggested order of work

1. Fix the metadata projection (item 1) and add the missing test; it is a small, contained change with a security boundary at stake.
2. Fix `/grant … @global` (item 2) and the membership check (item 3); both are in one branch and easy to test.
3. Reorder `/edit` authorization and move media reference validation into `authorize_media_text` (item 4).
4. Correct the CORS sample (item 5) and the two architecture-doc claims (items 6, 14).
5. Replace `Data` clone/compare with a family-scoped change set (item 6), which also lets items 11 and 13 fall out naturally.
6. Add a `messages(time)` index or a grouped count for `/clean` (item 7).
7. Client polish: full-response-only SW caching with recency, hint truncation, portable media URLs (items 9, 10, 15).

## Test gaps this review would close

* Room manager reads another account's metadata at room scope (`/user get bob #lobby` as owner) — asserts the `@global` section hides quota/size/age/cleanup.
* `/grant bob admin @global`, `/grant @all admin @global`, and `/revoke bob admin @global` — asserts success, assignment, membership unchanged, and a single audit entry.
* `/grant bob user #room` by an actor with policy write and member rights but without `x:member.add` — asserts denial.
* `POST /api/media/uploads` or a message body referencing another user's media id — asserts the same “not found”-class error as an unknown id (no oracle).
* `/clean` with zero matches — asserts no revision bump (if item 13 is fixed).
* `/user set @all media.user_quota` from a room — asserts the selection set documented in `docs/commands.md` (the room-context case is untested).


## Implementation follow-up (2026-10-21)

The actionable findings above have been addressed in the working tree:

- **1–4:** Room-only readers no longer receive an `@global` metadata section.
  Explicit global group grants work, bulk grants produce one audit entry, and
  admitting a room member requires invite/delegation authority and records the
  membership change. Message media checks now reject unknown, pending, expired,
  foreign, or cross-conversation references before mutation with `Upload not found.`;
  the transaction repeats the same defensive check. Edits check ownership first.
- **5–7:** The CORS example includes all hint headers. Administrative rollback uses
  family-scoped metadata snapshots without message bodies, plus a mutation flag;
  stdin/sudo reads reuse the read fast path. Cleanup counts use batched aggregate
  queries against the **already-existing** `messages_time` expression index. The
  original report's claim that this index was missing was incorrect; no schema
  migration is required. Working message deques were transient, not the retained
  page cache, so the original cached-history clone estimate also overstated the cost.
- **8:** First-DM directories and upload reservations commit in one transaction
  after size/quota/rate validation. Rejection leaves neither directory nor upload.
  Allocation authenticates before exposing storage configuration or hint errors.
- **9–10:** Media cache stores complete responses only, refreshes recency on hits,
  serves ranges from whole objects, and uses actual response lengths. Every
  request still reauthorizes through `/info` deliberately; no authorization TTL
  delays revocation. Filename hints truncate only at whole encoded code points.
- **11–13:** Bulk metadata key-cap checks run before any mutation. Storage quota
  reads use the metadata resolver. Existing quota shorthand remains compatible:
  current-view `@all` selection writes the global root; comments, docs and tests
  make that distinction explicit. Empty cleanup does not write/audit/bump revision.
- **14–15:** Account rename discards transient message rows after the durable
  rewrite; history cache revisions already invalidated retained pages. Docs now
  match metadata projection, rollback, allocation, and cache behavior. A new server
  instance clears unsent attachments with reattachment guidance. PUT hint headers
  derive from the signed header map. Posted media Markdown uses portable canonical
  root-relative paths, accepted narrowly by both link and image renderers. Cleanup
  waits and active storage work are interrupted by shutdown notification. Room
  routing strips `#` only from the room argument, preserving other tokens.

The documented startup deletion policy for unsent uploads and compatible quota
shorthand are retained. Legacy command aliases remain accepted. A speculative
scope-label index and a wholesale parser rewrite were not needed for these fixes;
`room_scope` already uses a keyed map lookup. No benchmarks were run.

Regression coverage is in `server/src/engine/review_tests.rs`, the web HTTP tests,
and the Markdown/media/cache/upload-hook tests. `auto/check.sh` runs the required
source checks and tests; benchmark cases remain ignored.
