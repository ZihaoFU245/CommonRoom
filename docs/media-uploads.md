# Media uploads

The browser uploads directly to private S3-compatible object storage. Chat owns
identity, authorization, pending metadata, message references and retention;
storage owns bytes. The transport uses `rust-s3`, not the official AWS SDK.
RustFS is the local integration backend. AWS S3 and Cloudflare R2 use the same
PUT/HEAD/CopyObject/range-GET/GET/DeleteObject adapter; provider accounts have not
been live-tested by the local suite.

## Authorization and upload flow

Uploads require conversation read/send permissions **and** `w:media.upload`.
Room user/admin groups and private-chat participants include uploads by default,
using server media limits.
The room owner can revoke one user's upload ability without removing their text
or read access. A room restriction also blocks explicit/global media grants there;
su retains full authority.
Use the existing policy commands:

```text
/revoke bob #files w:media.upload
/grant bob #files w:media.upload
/grant alice @global w:media.upload
/user set bob media.max_size 25m #files
/user set bob media.max_age 7d #files
/user set archivist media.max_age -1 #files
/user set bob media.user_quota 100m @global
```

Grant/revoke only operate on permissions. User metadata supplies maximum file
bytes, retention (s/m/h/d/w or -1 for permanent), and total account bytes. Room
values override global values, then server defaults. Size/quota remain within
server ceilings. `/grant` clears the room upload restriction; `/revoke` persists
it across reconnects and renames. Only su may set or remove metadata limits; admins and room managers have
inspection access only. `/user get bob #files` separates the global root from the room branch. Each
value shows whether it is set here, inherited from `@global`, or a server default.
Account-wide `media.user_quota` appears only in the global section.

Allocation persists a reservation before returning a five-minute presigned PUT
URL, signing the exact declared size and `application/octet-stream`. Clients send
bytes directly without chat cookies. Storage success is followed by idempotent
server completion: HEAD the staging object, copy to a server-only final key,
verify size again and classify at most 4 KiB of the file. Raster images
(PNG/JPEG/GIF/WebP/AVIF) render inline. Recognized MP4/WebM/Ogg video and
MP3/AAC/M4A/Ogg/WebM/WAV/FLAC audio use native browser players. The container's
codec must still be supported by the receiving browser; unsupported files retain
a download link. No decoding, transcoding, thumbnail generation or new decoder
dependency is involved. Unknown signatures remain application/octet-stream.
This is signature classification, not a full decode or malware scan.

Audio/video and HTML/SVG use ordinary Markdown links with a display-hint title
(e.g. `"media:video"`). Only exact same-origin, base-path-correct media links can
activate attachment controls. Hints never authorize access or determine the
trusted MIME. Document preview requests `/api/media/UUID/info`, which checks
publication, expiry and current conversation read permission without S3 I/O.
Audio/video use the authenticated file endpoint directly; its verified MIME and
current authorization checks apply before redirecting to storage.

Native audio/video controls appear immediately with no extra Play button, no
autoplay attribute and `preload="none"`. The browser defers file loading until
its Play control is used; no preliminary metadata request is needed. Preload is
a browser hint rather than an access restriction. The app refreshes a failed S3
redirect through the authenticated file endpoint, preserving playback position;
repeated failures before successful playback stop with Retry/download controls.
Seeking uses S3 range requests. `?download=true` requests attachment disposition
for the original file instead of inline playback.

HTML and SVG remain attachment downloads and are never navigated as inline
application documents. Each file shows its name beside Show text/Render HTML;
either action fetches at most 1 MiB of UTF-8 text and opens the chosen view in
one click. Switching views reuses the source. The download link stays below the
preview. Rendering respects the browser-local Allow HTML setting and uses the
same sandbox and network-denying CSP. The main app never inserts uploaded HTML/SVG as
DOM markup. Preview source is released when the virtual row unmounts. Older
uploads retain their stored classification; re-upload to use new formats.

The copy prevents a reused upload URL overwriting a published attachment.
Completion and cleanup serialize mutation of the same object; engine locks are released
before network I/O. Current identity/grants are rechecked after verification.

The composer holds a separate attachment card/preview with Remove, while text
remains editable. Pasting an image from the clipboard attaches it through the
same upload path as the attach picker: the paste is consumed, no Markdown or
image URL reaches the textarea, and an unnamed screenshot receives a filename
from its image type. The picker and pasted images share one availability rule,
so both are refused while console view, read-only access, an in-flight upload, a
pending send, an existing attachment, or a `/command` draft applies. A paste
without an image file keeps the browser's ordinary text behavior.
Upload/verification blocks every send path. Sending combines
text and an application media path as Markdown; a media-only message is allowed.
Raw attachment Markdown never enters the textarea. Preview object URLs are local
and released on removal, publication and navigation. Completed but unsent media
still consume pending slots. A rejected send retains the attachment/draft; an
uncertain acknowledgement asks the user to inspect history before retrying.
There is no automatic retry of an uncertain browser message.

A new private conversation can start with media: allocation accepts
`@direct:username`, authorizes creation, and commits an empty stable pair together
with its upload reservation after size, quota and rate checks. Rejected allocations
leave no private directory behind. Successfully reserved but unused pairs persist
like other empty private conversations and contain no message/unread event. Ordinary existing conversation IDs also work.

Messages and references commit atomically. Pending publication ends only when a
message commits. Media records remain for authorization and cleanup. Other readers
may reuse a published file in the same conversation if their upload policy allows
it; cross-conversation and foreign unpublished references are rejected. Edits,
retraction and message retention reconcile references transactionally. Preserving
an already-expired reference while editing surrounding text is allowed.

Downloads require current read access, an unexpired verified object and a retained
message reference. Chat redirects to a signed GET valid for at most 60 seconds,
capped by remaining media life. Redirects/storage responses use `no-store`. A scoped `media-cache.js` service worker
persists immutable bytes under the stable application media URL. Every reuse first
checks `/info` with the current session (no S3 I/O), including every seek/range
request. Authorized readers can retrieve expired metadata with
`/info?include_expired=true`; normal `/info` and file reads continue to reject
expiry, and the worker never serves expired cached bytes. The UI replaces expired images,
players, previews and file links with a themed “Attachment expired” card showing
the original filename. Expiry is rechecked while mounted and on tab resume.
Expiry, retraction and lost access still stop new byte reads immediately. This
intentionally pays one authorization/database request per range; there is no auth TTL. Reloads, preview/download switches and playback retry URLs
reuse those bytes. Full cached objects serve byte ranges locally. Only complete
200 responses are cached; partial playback responses stream without occupying cache
entries. Cache hits refresh eviction recency. The cache holds at most 128 entries/256 MiB;
larger responses stream normally, and browser eviction or disabled storage can
require another download. Cache Storage retains local downloaded copies across
sessions; it contains no credentials and never supplies an offline authorization
fallback. Clear site data to erase these local copies.
A downloaded copy cannot be revoked; an issued URL can remain usable until expiry.

## Configuration

Connection settings are independent of chat admission/retention policy. Save
non-secret settings in `data/config.json` while stopped, then restart:

```json
{
  "s3": {
    "endpoint": "https://s3.us-east-1.amazonaws.com",
    "public_endpoint": null,
    "bucket": "your-private-bucket",
    "region": "us-east-1",
    "path_style": false
  },
  "media": {
    "max_size": "25m",
    "max_age": 604800,
    "pending_ttl": 86400,
    "cleanup_interval": 86400,
    "cleanup_batch": 32,
    "user_quota": "1g",
    "pending_limit": 8,
    "allocations_per_minute": 30,
    "sticker_max_size": "2m",
    "sticker_limit": 200
  }
}
```

Times are seconds. `max_age` is the default maximum age, not a ceiling on explicit
authorized user retention metadata. `-1` disables age expiration, but not quotas or
cleanup after message references disappear. `max_size` is a hard chat ceiling;
client validation reads it from the API. Per-grant size restrictions are also
checked server-side. Pending slots include verified unsent objects; byte quota
remains reserved until storage deletion succeeds. The default limit is 25 **MiB**.
`sticker_max_size` (default 2 MiB, also capped by `max_size`) and `sticker_limit`
(default 200, at most 10000) bound each account's [sticker library](#stickers).

| Environment variable | Purpose |
| --- | --- |
| `CHAT_S3_ENDPOINT` | Override `s3.endpoint`; its presence can enable storage without saved S3 settings |
| `CHAT_S3_PUBLIC_ENDPOINT` | Browser-reachable endpoint; defaults to internal endpoint |
| `CHAT_S3_BUCKET` | Override `s3.bucket` |
| `CHAT_S3_REGION` | Override region; defaults to `us-east-1` |
| `CHAT_S3_PATH_STYLE` | `true` or `false`; defaults to true without saved settings |
| `CHAT_S3_ACCESS_KEY` | Required service access key; environment only |
| `CHAT_S3_SECRET_KEY` | Required service secret key; environment only |
| `CHAT_S3_SESSION_TOKEN` | Optional temporary credential session token |

Storage is disabled when neither `s3` nor `CHAT_S3_ENDPOINT` is configured.
The earlier draft's `CHAT_MEDIA_*` connection variables have been replaced by
`CHAT_S3_*`. Endpoint environment overrides are process-local, not persisted.
Endpoints must be origins with no credentials/path/query/fragment. Production's
public endpoint requires HTTPS and must differ from the chat application origin.
Both endpoints must reach the same bucket. Preserve signed Host/path/query through
proxies. Do not use a public CDN/custom-domain endpoint that lacks S3 signing.

| Backend | Endpoint example | Region | Path style |
| --- | --- | --- | --- |
| RustFS local | `http://127.0.0.1:19000` | `us-east-1` | true |
| AWS S3 | `https://s3.us-east-1.amazonaws.com` | actual bucket region | false |
| Cloudflare R2 | `https://ACCOUNT_ID.r2.cloudflarestorage.com` | `auto` | true |

R2 lacks S3 object-tagging support, so uploads do not require tags or AWS-specific
lifecycle APIs. No provider name switch is needed. S3-compatible services must
support the object operations above, presigned response-header overrides and CORS.

## Retention and storage lifecycle

Object age starts at successful verification, matching final-copy creation rather
than message activity. Sending, editing or reusing a file never resets expiry.
Settings changes apply prospectively; each upload saves its selected retention.

Keys use `uploads/UUID` for staging and `media/7d/UUID`, `media/1d/UUID`,
`media/3600s/UUID` or `media/permanent/UUID` for verified content. Unique keys are
never rotated/reused. Operators can configure **prefix-based** storage lifecycle
rules for these classes on AWS, R2 or RustFS, without relying on object tags.
For example, expire `uploads/` after one day and `media/7d/` after seven days;
exclude `media/permanent/`. Configure matching rules for any additional classes.
Lifecycle rules are provisioned separately using each provider's console/API;
chat does not change bucket policy or lifecycle configuration.

Lifecycle deletion is asynchronous and day-rounded; it is a storage cleanup
backstop, not exact access enforcement. Chat rejects expired reads/new references
immediately and its worker also deletes expired objects through standard S3 APIs.
This keeps arbitrary grant ages and `-1` portable even without lifecycle rules.
Do not configure storage retention shorter than the corresponding chat allowance.
For versioned buckets, also remove noncurrent versions; delete markers alone do
not reclaim bytes. Use a dedicated bucket/prefix for this application.

On each server start, before accepting uploads, tracked objects without retained
message or selected avatar references become eligible for deletion only after the
pending grace period. Fresh unfinished and completed-but-unsent uploads keep the
same grace period across restarts. Already claimed deletions resume immediately.
Shared attachments survive while any message still references them. Cleanup uses
authoritative `message_media` rows rather than scanning S3, and needs no bucket-list
permission. Unknown objects created outside the tracked upload protocol remain the
operator's responsibility. The browser still clears unsent attachments when it
receives a new server instance and asks the user to attach again.

Cleanup starts immediately and repeats every 24 hours by default. An explicit saved
`cleanup_interval` is preserved. Published attachments become eligible as soon as
their last reference disappears; new unsent uploads retain the pending grace period.
Cleanup's configurable timer handles expired published objects even while messages
still reference them, abandoned/pending uploads and unreferenced files. It claims
deletion in SQLite before storage I/O. Failed deletion retries without releasing
quota; oldest-attempt ordering prevents one failing object starving other work.
Expired reference tombstones survive while messages retain them. Early deletion leaves staging recovery pending until the upload's grace period
ends, catching PUTs that finish after a restart or cancellation. Staging copies
are swept after the pending timeout, well beyond URL expiry. Configure staging
lifecycle as a backstop for very late in-flight PUTs or process crashes.

Chat and storage size limits are independent. Chat may permit 120 MB while a
storage ingress capped at 100 MB rejects a 110 MB upload. Such rejection never
makes an attachment sendable. An arbitrary configurable per-file cap is not a
universal S3 bucket feature: enforce it in the selected service/ingress where
needed. Completion still verifies actual size.

## Local development

Manually download/extract the RustFS executable for your OS/architecture into
`tmp/rustfs` and make it executable. Neither launcher downloads RustFS:

```sh
rtk proxy ./auto/start-s3.sh
```

The S3 API binds `127.0.0.1:19000`, console `127.0.0.1:19001`. Runtime cwd,
data/log/temp files stay in ignored `tmp/rustfs-local/`. After stopping RustFS,
`auto/clean.sh` removes that runtime directory, including all local buckets;
it preserves the manually installed `tmp/rustfs` binary and other `tmp/` files.
The launcher clears
inherited output-path settings and confines filesystem writes using macOS
`sandbox-exec` or Linux `bubblewrap` (install it first). It fails if confinement
cannot be established and rejects symlink runtime roots. It starts only RustFS.

`auto/debug.sh` configures chat for this local endpoint/bucket and matching dev
credentials (`cr` / `pass`) automatically, but does **not** start or stop RustFS.
Set `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY` before launching both scripts to
override those credentials consistently. For a loopback
HTTP endpoint with path-style addressing, debug runs `chat --setup-local-s3` to
create the private bucket if missing and apply browser CORS on every start. This
setup command exits before opening chat data and refuses remote endpoints.
Launch RustFS in its own terminal first, then run debug. Explicit `CHAT_S3_*`
variables override the debug defaults. Remote buckets remain operator-managed.
If local setup fails, debug prints the error and starts chat with uploads enabled;
start RustFS and restart debug to retry bucket/CORS setup. After changing server configuration, restart debug and refresh the
browser so the upload capability is fetched again.

Debug creates the **private** `commonroom-draft` bucket automatically with this CORS policy (configure it manually when running chat without debug):

```json
{"CORSRules":[{
  "AllowedOrigins":["http://127.0.0.1:5173","http://localhost:5173"],
  "AllowedMethods":["PUT","GET","HEAD"],
  "AllowedHeaders":["content-type","x-amz-meta-client-filename","x-amz-meta-client-content-type","x-amz-meta-client-label","x-amz-meta-client-client"],
  "ExposeHeaders":["ETag"],
  "MaxAgeSeconds":300
}]}
```

Start chat with isolated test data, then provision accounts/rooms and grants using
stdin, and run the frontend normally (room members can upload immediately):

```sh
rtk proxy env CHAT_DATA="$PWD/tmp/media-chat" \
  CHAT_S3_ENDPOINT=http://127.0.0.1:19000 \
  CHAT_S3_BUCKET=commonroom-draft \
  CHAT_S3_ACCESS_KEY=cr \
  CHAT_S3_SECRET_KEY=pass \
  cargo run --locked
```

Development credentials must never be used for public storage. Keep storage's
administration console private. Service credentials need object read/write/delete
on `uploads/*`, `media/*`, `avatars/*`, and `stickers/*`; clients get no listing/deletion/final-key writes.

## Deployment and verification

Configure exact frontend CORS origins. Add the storage public origin to UI Nginx
`connect-src` and `media-src`; with virtual-host addressing include the bucket host (for example
`https://BUCKET.s3.us-east-1.amazonaws.com`). Backend CSP derives the actual
bucket origin automatically. Attachment previews also require
`blob:` in `img-src` (included in the supplied Nginx/backend headers). Storage must
set `X-Content-Type-Options: nosniff`. Keep the bucket private and avoid public
caching or listing. Serve storage from a separate origin without chat cookies.

Provider-specific bucket, CORS, credential, and lifecycle setup (AWS S3, Cloudflare R2,
self-hosted RustFS) is in [Deployment](deployment.md#object-storage).

SQLite HEAD is v20, with an atomic v19 credential upgrade. Other SQLite versions
are rejected unchanged; PostgreSQL has an independent current schema.

```sh
rtk cargo build --locked
rtk proxy node web/scripts/media-smoke.mjs
rtk proxy ./auto/check.sh
```

The real RustFS smoke test uses `tmp/rustfs`, isolated temporary process cwd/data,
a private test bucket and ephemeral ports, and removes its own workspace. It
checks signing, CORS, HTTP/WebSocket flow, verification, private download access,
message references and retraction under a non-root base path. No external storage
account is used. Unit tests cover grants, pending slots, retention, cancellation,
cleanup claims, reference rollback, restart, contention and abrupt-process recovery. Provider URL
construction tests supplement local integration; production AWS/R2 credentials,
actual CORS/ingress policy and lifecycle behavior require deployment verification.

Storage outages do not hide configured upload controls. Allocation authenticates
and validates the request, then performs a signed object HEAD with a three-second
timeout before reserving pending quota or issuing a URL. Connection failures and
storage server errors return HTTP 503: “Upload rejected: media storage is
unavailable. Try again later.” Authorization is rechecked after that I/O. The
probe needs no listing/write permission; missing-object 403/404 responses prove
reachability, while S3 still enforces actual PUT permissions and size constraints.

## Availability and bounded work

Media holds the shared API admission permit only during blocking auth/SQL work.
S3 uses eight independent outbound slots; completion and cleanup lock only the
same media ID. Saturation or duplicate completion returns 503 promptly, leaving
other chat operations available. Storage failures return 503; invalid sizes or
missing staging objects return 400. Zero sizes are rejected in the transport too.
Completion has a 100-second server budget below the browser's 120-second deadline.
It uses three S3 calls: staging HEAD, copy, then bounded range GET verifying the
copied object's total size and type. A provider ignoring large-object Range is
rejected before buffering. Diagnostics omit signed request URLs.

Concurrent allocation probes share in-flight work only. Later requests always
probe again; there is no stale health cache that allocates while storage is down.
Downloads/previews retain no-store on the network and live authorization rechecks;
the browser worker explicitly stores immutable bytes locally.

Cleanup drains batches with up to four concurrent object jobs within a 20-second
sweep budget. Failures or exhausted budgets retry within 60 seconds (or the configured
interval when shorter), so a startup backlog does not wait another day. It batches each object's distinct
keys with DeleteObjects, checks every returned key/error, and falls back to
individual deletion for unsupported batch APIs (405/501). A failed or incomplete
delete retains the row and quota. Final/staging retry timestamps prevent failed
objects monopolizing subsequent batches. V15 adds partial due-set/tombstone
indexes, a covering quota index, staging retry state, and reference-removal
tracking. Mutable SQL columns are authoritative over legacy JSON state copies.


Single PUT/copy uploads have a conservative portable 5,000,000,000-byte ceiling;
provider/ingress limits may be lower. A PUT URL must be used within five minutes;
that expiry does not terminate an already admitted transfer. The client uses a
separate 15-minute transfer timeout, deduplicates progress at most 10 times per
second, and creates local image previews only for files up to 4 MiB.

## Client object hints

Allocation accepts optional `hints`: filename, content-type, label, and client.
Each value is 1–256 printable ASCII bytes with no surrounding spaces; encode
Unicode filenames using percent encoding. Unknown keys, controls, extra entries,
and oversized values are rejected before any quota reservation or storage I/O.
The browser sends a percent-encoded filename, its declared MIME when available,
and `commonroom-web` as client. These are inspection hints supplied by the client.

The returned PUT headers include signed `x-amz-meta-client-<key>` fields. Send
those headers exactly, and allow the four `x-amz-meta-client-` header names in
bucket CORS, as listed in [Deployment](deployment.md#bucket-cors). Finalization
uses [CopyObject metadata replacement](https://docs.aws.amazon.com/AmazonS3/latest/API/API_CopyObject.html)
to keep only the validated allocation hints, dropping arbitrary staging metadata.
Hints never set trusted MIME, authorization, size, quota, retention, object paths
or download response headers; normal server verification remains authoritative.

## Account avatars

Avatar uploads share this private bucket, signing, quota/rate limits and cleanup
worker, but do not require room media permissions. The authenticated account owns
its profile. Settings accepts local PNG/JPEG/WebP sources up to 20 MiB and 40
megapixels, crops and resizes to at most 256×256, and encodes WebP at 80% quality
(PNG fallback). It reduces dimensions further if needed to fit 64 KiB before
uploading; the server caps new allocations at 64 KiB. Previously saved avatars
remain readable. External URLs load the original bytes without this size limit.
Completion accepts raster PNG/JPEG/WebP. The original
rectangle is never uploaded. Final files use `avatars/<account-id>/<upload-id>`;
include `avatars/*` in storage credentials and backups and exclude it from expiry
lifecycle rules. Saved avatars have permanent retention, remain protected during
startup/daily cleanup, and stop receiving new download URLs when replaced or
removed. Browser loads and updates propagate through the public avatar projection
in snapshots. External HTTPS avatar URLs work even with storage disabled.
See [Protocol](protocol.md#user-avatars).

Failed S3 availability probes are reused for at most one second so repeated
allocations fail promptly during an outage. Concurrent probes share in-flight
results; successful probes are not reused by later requests. Downloads sign once
and carry a stable-account ticket through signing. Before redirecting, the server
rechecks credentials, current read permission, expiry and a writer-change stamp;
it reloads metadata/references when that stamp changes. Shared-reader media
cleanup writes also invalidate the stamp. No global API permit spans S3 I/O.

## Stickers

The composer's sticker button, shown beside the attach button wherever attaching
is available, opens the caller's private library. Any active account can add
stickers while storage is configured. PNG/JPEG/WebP are resized in the browser
to at most 512 px and encoded WebP (PNG fallback); GIFs keep their original bytes
so animation survives, and are rejected above `sticker_max_size`. Completion
verifies PNG/JPEG/GIF/WebP bytes and copies them to
`stickers/<stable-account-id>/<upload-id>`. Library entries use conversation
`@sticker`, permanent retention and the shared byte quota and rate limits; they
are never treated as unreferenced. Deleting an entry queues its object for the
cleanup worker. Include `stickers/*` in storage credentials and backups and
exclude it from expiry lifecycle rules.

Sending copies the sticker into the target conversation as an ordinary upload
(normal `w:media.upload` permission, room retention, quota and cleanup), so
deleting a sticker never removes messages already sent. Right-clicking a sent
sticker offers Add to my stickers, which copies it into the viewer's library.
See [Protocol](protocol.md#stickers).

## Bot transport

Machine uploads/downloads and avatar changes use explicit `/api/bot/v1/`
counterparts. The browser `/api/media/*` and `/api/avatars/*` routes reject bearer
headers. Bot handlers share the same storage, quotas, grants, ownership and
finalization services. See [Bot media and profiles](bots.md#media-and-profiles-v1-extension)
for the endpoint mapping, worker operation/conversation scopes and redirect rules.
