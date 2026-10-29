# Deployment

The instructions below default to SQLite. To use PostgreSQL, set
`database.backend` to `postgres` in `config.json`, put `CHAT_DATABASE_URL` in the
service environment, and follow [PostgreSQL](postgresql.md)
for initialization or transfer of an existing HEAD SQLite database. Retain the
local data folder for configuration and worker state; PostgreSQL requires its own
database backups in addition to that folder and the object store.

This guide takes a release build to a public HTTPS site: artifacts, first start,
configuration, a reverse proxy, and optional S3-compatible media storage.

Related docs: [Development](development.md) for the local loop, and
[Media uploads](media-uploads.md) for upload permissions, limits, and cleanup
internals. [Data](data.md) covers persistence and backups; [Bots](bots.md) and
[Agent development](agent-development.md) cover machine clients.

## Contents

- [Build](#build)
- [Install and first start](#install-and-first-start)
- [Configuration](#configuration)
- [Run as a service](#run-as-a-service)
- [Nginx](#nginx)
- [Cloudflare in front](#cloudflare-in-front)
- [Object storage](#object-storage)
- [External agent workers](#external-agent-workers)
- [Upgrades and backups](#upgrades-and-backups)
- [Post-deploy checks](#post-deploy-checks)
- [Troubleshooting](#troubleshooting)

## Build

A release consists of two artifacts:

| Artifact | Contains | Installed as |
| --- | --- | --- |
| `chat` | Rust server binary | `/srv/commonroom/chat` |
| `ui.tar.xz` | `web/dist/` | `/srv/commonroom/dist/` |

Build both from the repository:

```sh
rtk pnpm --dir web install
rtk proxy ./auto/check.sh
rtk pnpm --dir web build
rtk cargo build --release
rtk proxy cp target/release/chat ./chat
rtk proxy tar -cJf ui.tar.xz -C web dist
```

`rtk proxy ./auto/build.sh` is the one-shot release build: it installs dependencies and
produces `./chat` and `./ui.tar.xz`. It skips source checks and tests, so run
`auto/check.sh` separately. On first use it generates missing lockfiles; later builds
use frozen/locked dependencies.

To build only the server, without Node or web assets:

```sh
rtk cargo build --release --locked
```

Debug builds (`rtk proxy ./auto/build.sh debug`) produce `target/debug/chat` and
`web/dist-debug/`. `rtk proxy ./auto/debug.sh` serves the Vite preview on
`127.0.0.1:5173`, proxies `/api` and `/ws` to `127.0.0.1:3000`, and stops both
processes together. Debug builds are not for deployment.

The binary and UI can ship independently when their protocol versions match; a release
build pairs them. Static files come from Nginx, so the server does not need Node or
pnpm installed.

## Install and first start

Create the install directory and extract the UI archive there:

```sh
mkdir -p /srv/commonroom
tar -xJf ui.tar.xz -C /srv/commonroom
```

Resulting layout:

```text
/srv/commonroom/
  chat
  dist/
  data/          # created on first start
```

The first production start must know the public origin, because release builds enforce
HTTPS and origin checks:

```sh
CHAT_ORIGIN=https://chat.example.com ./chat
```

Later starts need no variables: `./chat`. `CHAT_ORIGIN` must be an HTTPS origin with no
path or trailing slash.

The listener defaults to `127.0.0.1:3000` and serves only `/api/` and `/ws`. Data lives
in `./data` unless `CHAT_DATA` says otherwise. The directory is created on first start,
set to mode `0700` on Unix, and holds `config.json` and `chat.sqlite`. Only one process
may use a data folder at a time.

Release binaries always run with production security (HTTPS origins, secure cookies,
forwarded-header checks). Reusing an existing development data folder therefore requires
`CHAT_ORIGIN` once; the value is then saved.

Logs go to stdout. To keep the interactive console while logging to a file:

```sh
./chat > chat.log 2>&1
```

Closing stdin leaves the web server running. `Ctrl+C` or `SIGTERM` stops it gracefully
and checkpoints SQLite. No account exists on first start: create the first admin from
the console with `/user new NAME PASSWORD admin` (see [Commands](commands.md)).

## Configuration

Settings are a JSON object in `data/config.json`. Stop the server before editing; values
are read at startup.

```json
{
  "production": true,
  "bind": "127.0.0.1:3000",
  "trust": "127.0.0.1",
  "set_real_ip_from": "X-Forwarded-For",
  "base_url": "/commonroom/",
  "origins": ["https://chat.example.com"],
  "max_users": 64,
  "max_rooms": 64,
  "max_messages": 1000
}
```

| Key | Default | Purpose |
| --- | --- | --- |
| `direct_bot_http` | `false` | Opt in to direct versioned bot HTTP only from a private/loopback `trust` peer. |
| `avatar_hosts` | `[]` | Exact canonical HTTPS avatar hosts; no wildcards, schemes, paths or ports. |
| `production` | `false` | Production security. Release binaries enforce it regardless. |
| `bind` | `127.0.0.1:3000` | HTTP listener for `/api/` and `/ws`; static files are not served here. |
| `trust` | `127.0.0.1` | The single proxy IP allowed to connect. Enforced in production. |
| `set_real_ip_from` | `"X-Forwarded-For"` | Header carrying the real client IP for login throttling; `null` disables it. |
| `base_url` | `"/"` | Path prefix for the API, WebSocket, and UI URLs. |
| `origins` | local dev origins (`localhost:5173`, `localhost:3000`) | Browser origins accepted for login, logout, read acks, and WebSocket auth. |
| `max_users` | `64` | Accounts, including disabled ones. |
| `max_rooms` | `64` | Rooms. |
| `max_messages` | `1000` | Retained messages per room and per private conversation; the oldest are evicted. |

`bind` is plain HTTP; terminate TLS at the proxy and keep the listener off public
interfaces.

`base_url` is a path, not a URL. `/commonroom/` produces `/commonroom/api/...`,
`/commonroom/ws`, and asset URLs under that prefix. Path segments may contain letters,
digits, `-`, and `_`; a missing trailing slash is added.

`trust` must be the proxy's exact IP as seen on the socket; forwarded headers cannot
override it. Configs written before the field existed use `127.0.0.1`.

`set_real_ip_from` names the header that carries the client IP. It is a header name, not
the Nginx directive of the same name. It accepts exactly one IPv4 or IPv6 address (a
malformed value, duplicate, or comma list rejects login with 400). IPv4-mapped
IPv6 addresses share one throttle bucket. The value is read at startup, so restart after
changing it.

`origins` entries are scheme, host, and optional port, never a path: use
`https://chat.example.com`, not `https://chat.example.com/commonroom/`. Production
requires HTTPS. To change the production origin later, start once with the new
`CHAT_ORIGIN`; it is saved. The storage public origin must not be listed here; the server
refuses to start if one matches.

Limits are positive integers. Lowering an account or room limit preserves existing data
but blocks new creation until usage is below it. Lowering `max_messages` also trims
retained history at the next start. The in-memory page caches default to 1,000 entries per
conversation and 10,000 entries / 32 MiB globally, independently of `max_messages`.
Tune global cache budgets with `database.history_cache` and PostgreSQL idle
reclamation with `database.postgres_pool.idle_timeout_seconds`; see
[PostgreSQL](postgresql.md#runtime-and-backups). Hard
ceilings are 256 users, 1,024 rooms, and 1,000,000 retained messages; larger values are
rejected at startup.

### Environment variables

| Variable | Purpose |
| --- | --- |
| `CHAT_DATA` | Data folder. Default `./data`. |
| `CHAT_DATABASE_URL` | PostgreSQL connection secret, required only when the selected config sets `database.backend=postgres`; never saved. |
| `CHAT_ORIGIN` | Public HTTPS origin, e.g. `https://chat.example.com`. Required on the first production start and saved. |
| `CHAT_BIND` | Listener address. Saved when overridden. |
| `CHAT_TRUST` | Proxy IP. Saved when overridden. |
| `CHAT_BASE_URL` | Path prefix. Saved when overridden. |
| `CHAT_PRODUCTION` | `1` enables production security in a debug build. Release builds are always production. |
| `RUST_LOG` | Log filter. Default `chat=info,tower_http=info`. |
| `CHAT_S3_*` | Object storage; see [Object storage](#object-storage). |

Environment values override saved settings. `bind`, `trust`, `base_url`, and `origin`
are written back to `config.json`. Release builds cannot be downgraded to
non-production, and pointing a fresh release build at old development data requires
`CHAT_ORIGIN` once.

Login throttling allows 10 failed attempts per client IP and 20 per username per minute. When a proxy hides client
IPs, see [Cloudflare in front](#cloudflare-in-front).

Unknown configuration keys, including keys within `media` and `s3`, stop startup
with an error so misspellings cannot silently change security settings. Startup
preserves the file and its formatting when normalized values are unchanged.
Changes are saved through an exclusive temporary file, synced before atomic
replacement; Unix also syncs the parent directory.

Login rejects malformed or repeated configured client-IP headers from the trusted
proxy with 400. The proxy must overwrite the header with one verified IP rather
than append a forwarding chain. Set `set_real_ip_from` to `null` when using socket
addresses directly. Failure windows are shared across connections and limit both
IP addresses and usernames; see [Protocol](protocol.md).

## Run as a service

Example systemd unit; run the binary as its own unprivileged user, with the working
directory set to the install root so `data/` resolves there:

```ini
[Unit]
Description=Chat server
After=network.target

[Service]
Type=simple
User=chat
Group=chat
WorkingDirectory=/srv/commonroom
EnvironmentFile=/etc/chat.env
ExecStart=/srv/commonroom/chat
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
```

`/etc/chat.env`, owned by root with mode `0600`:

```sh
CHAT_ORIGIN=https://chat.example.com
CHAT_DATA=/srv/commonroom/data
# CHAT_S3_ACCESS_KEY=...
# CHAT_S3_SECRET_KEY=...
```

Create the environment file before enabling the unit. Under systemd stdin is `/dev/null`,
so the console is unavailable: provision accounts and bot tokens during an interactive
run, or after stopping the service, start the binary once with the same `CHAT_DATA` to
use the console. `systemctl stop` sends `SIGTERM`, which performs the graceful shutdown.

## Nginx

`deploy/nginx.conf` is a location-only include for the UI paths. Put it inside an
existing HTTPS `server {}` block in `http {}`; that block owns certificates and the
HTTP-to-HTTPS redirect. Nginx does not allow `location` directly in `http`.

```nginx
server {
    listen 443 ssl;
    server_name chat.example.com;

    # TLS configuration...

    include /etc/nginx/snippets/commonroom.conf;
}
```

The sample assumes `/commonroom/` and assets in `/srv/commonroom/dist/`. If `base_url`
changes, update the location prefixes to match; if the install root changes, update the
`alias` paths.

The `/assets/` location caches Vite content-hashed files for one year with
`immutable`. HTML and `media-cache.js` use `max-age=0, stale-while-revalidate=7200`
so supporting caches can serve stale content while refreshing in the background.
Cloudflare must make HTML eligible for caching and respect this origin policy;
keep authenticated API paths excluded. Purge the HTML and worker when a deployment
must take effect immediately. Preserve the security headers in both static
locations when adapting the sample.
Publish hashed assets before the HTML and retain previous hashed assets while stale
HTML or existing tabs can still reference them, including lazy-loaded chunks.

### Paths and routing

- `/commonroom` redirects with 308 to `/commonroom/`, so relative asset URLs resolve.
- `proxy_pass` has no trailing slash, preserving the `/commonroom/` prefix the server
  expects.
- Only `/api/` and the exact `/ws` path reach the Rust listener; everything else is
  static. Unknown files return 404 rather than falling back to `index.html`.
- The API limit is `client_max_body_size 8k` because uploads bypass the server. The
  completion call may take up to `proxy_read_timeout 130s` while it talks to storage.
- WebSocket reads and sends time out after 90s, and buffering is off.

For root hosting (UI at `https://chat.example.com/`): set `base_url` to `"/"`, use
locations `/api/`, `/ws`, and `/`, and remove the prefix redirect. Keep `/api/` and
`/ws` more specific than `/`.

### Forwarded headers and trust

The release server rejects requests without `X-Forwarded-Proto: https`, except explicitly enabled direct
bot calls from the private trusted socket IP (`direct_bot_http`). The proxy must set all three headers on every
proxied path:

```nginx
proxy_set_header Host $host;
proxy_set_header X-Forwarded-Proto $scheme;
proxy_set_header X-Forwarded-For $remote_addr;
```

Use `$remote_addr`, not `$proxy_add_x_forwarded_for`: appending keeps a client-supplied
value first, which the server cannot distinguish from a real proxy. If another trusted
proxy sits in front of Nginx, configure `ngx_http_realip_module` there and keep `trust`
pointing only at the socket peer Nginx actually sees.

Set `trust` to Nginx's address (loopback when Nginx is local), bind the Rust listener to
a private interface, and never expose it directly; it does not terminate TLS. A trusted
same-machine bot may call `/api/bot/v1/` over plain HTTP only with
`direct_bot_http` enabled. Browsers always need the forwarded HTTPS signal.

### Headers and previews

Backend responses carry HSTS, CSP, and secure, HttpOnly cookies scoped to `base_url`.
Nginx must supply the same protection for static files, which is why the sample adds
`Strict-Transport-Security`, `X-Content-Type-Options`, `Referrer-Policy`, and a CSP to
the asset location.

Keep the main document CSP strict: `script-src 'self'`, `style-src 'self'`,
`frame-src 'self'`, `frame-ancestors 'none'`, and no `unsafe-inline`. `img-src` must
allow remote and `data:` images for message content, and `font-src 'self' data:` is
needed by KaTeX. The UI also redirects HTTP to HTTPS and uses `wss://` on production
pages.

`markdown-frame.html` renders one message's Markdown HTML inside a sandboxed frame. It
is generated into both UI builds and must be deployed with the assets. It needs its own
location, placed before the general asset prefix:

- CSP allows inline scripts and styles only in that sandboxed document and sets
  `frame-ancestors 'self'`; do not apply the main app's `frame-ancestors 'none'` or
  script policy to it.
- Frame URLs are versioned, so it is cached `public, max-age=31536000, immutable`.
- It keeps a `default-src 'none'` baseline, allows only `data:` images, media, and
  fonts, and blocks connections and nested frames. Content arrives by one-shot
  `postMessage` with its own meta CSP and sandbox, so it still works without these
  headers, but the Nginx copy must not weaken them.

If storage is enabled, the static CSP must also name the storage origin; see [Storage
headers and CSP](#storage-headers-and-csp). Validate changes with `nginx -t` before
reloading.

## Cloudflare in front

Cloudflare terminates public TLS and forwards to Nginx through the tunnel; the Rust
listener stays private.

- Enable **Always Use HTTPS** for the hostname, and leave WebSockets enabled.
- Route the public hostname to Nginx over HTTPS. A tunnel preserves the request path, so
  keep `base_url` at `/commonroom/` and forward it unchanged.
- Do not cache `/api/*` or `/ws` (or their base-path equivalents). Signed media URLs are
  short-lived and private; a shared cache in front of the API or storage is not
  supported.
- Keep `trust` equal to Nginx's socket peer, and keep Nginx overwriting
  `X-Forwarded-For` with `$remote_addr`. The server still requires
  `X-Forwarded-Proto: https`, which Nginx sets from `$scheme`.
- With Cloudflare in front, `$remote_addr` at Nginx is a Cloudflare edge address.
  Configure `ngx_http_realip_module` with Cloudflare's published ranges only if
  per-client login throttling and logging matter; otherwise every user shares one IP for
  the 10-failures-per-minute IP limit. Never trust ranges you do not control.
- WebSocket clients reconnect after a tunnel restart and resynchronize instead of
  replaying missed events.

Run an end-to-end check on the public hostname, not only on the origin: HTTPS redirect,
WebSocket upgrade, and login. References: [Tunnel routing][cf-routing], [Tunnel
FAQ][cf-faq], [HTTP headers][cf-headers].

[cf-routing]: https://developers.cloudflare.com/tunnel/concepts/routing/
[cf-faq]: https://developers.cloudflare.com/cloudflare-one/faq/cloudflare-tunnels-faq/
[cf-headers]: https://developers.cloudflare.com/fundamentals/reference/http-headers/

## Object storage

Media uploads are optional. Without storage configuration the site still runs but
uploads are unavailable. When configured, the browser uploads straight to a private,
S3-compatible bucket with short-lived presigned URLs; the chat server keeps metadata,
permissions, quotas, and cleanup. It never streams file bytes, so the server needs
reliable access to the bucket and the browser needs a reachable public endpoint.

AWS S3, Cloudflare R2, and self-hosted RustFS use the same adapter. Provider accounts are
not covered by the local test suite (only local RustFS is exercised), so verify a real
upload after any provider change. Permissions, limits, and the upload protocol are in
[Media uploads](media-uploads.md).

### How it fits

1. The browser asks the chat API for an upload slot.
2. The server reserves quota and returns a five-minute presigned `PUT` for
   `uploads/<id>`.
3. The browser uploads the exact declared size with
   `Content-Type: application/octet-stream`.
4. On completion the server checks the staging object, copies it to
   `media/<class>/<id>`, verifies size and type, and adds the message.
5. Reads redirect to a presigned `GET` valid for at most 60 seconds. No public bucket
   access or listing is used.

The server makes three storage calls per completion and runs up to 8 outbound storage
operations at once. Keep the API proxy timeout at 130s or more.

### Common setup

1. Create a private bucket. Do not enable public access, website hosting, or directory
   listing.
2. Create credentials that can `GetObject`, `PutObject`, and `DeleteObject` on
   `uploads/*`, `media/*`, and `avatars/*` in that bucket. Chat needs no bucket administration
   permission at runtime; bucket creation, CORS, and lifecycle rules are operator work.
3. Configure bucket CORS for browser uploads ([Bucket CORS](#bucket-cors)).
4. Set `s3.endpoint` (the server's route to storage). Set `public_endpoint` (the
   browser's route) only when it differs; it defaults to `endpoint`. In production the
   public endpoint must be HTTPS.
5. Give the server the access key and secret through the environment.
6. Add the public endpoint's origin to the UI's CSP ([Storage headers and
   CSP](#storage-headers-and-csp)).
7. Upload a file through the UI and verify it renders, downloads, and can be retracted.

The storage origin must differ from every value in `origins`; the server exits at startup
with `Media storage must use a separate origin from the chat application.` This keeps
chat cookies away from media responses and makes CSP scoping possible.

### Storage settings

Add an `s3` block to `data/config.json` (stop the server first):

```json
{
  "s3": {
    "endpoint": "https://s3.us-east-1.amazonaws.com",
    "public_endpoint": null,
    "bucket": "commonroom-media",
    "region": "us-east-1",
    "path_style": false
  }
}
```

| Key | Meaning |
| --- | --- |
| `endpoint` | Server-to-storage origin, e.g. `http://127.0.0.1:19000` for local RustFS. |
| `public_endpoint` | Browser-to-storage origin. Defaults to `endpoint`; HTTPS required in production. |
| `bucket` | Bucket name; no `/`, `?`, or `#`. |
| `region` | SigV4 region. Default `us-east-1`; R2 uses `auto`. |
| `path_style` | `true` puts the bucket in the URL path; `false` uses virtual-host style. Defaults to `true` when no saved setting exists, otherwise the saved value. |

Both endpoints must be origins: scheme and host, optional port, no credentials, query,
fragment, or path (except `/`). They must point at the same bucket. `public_endpoint`
must be HTTPS in production, and the server derives the bucket's public origin from it
for CSP headers.

| Variable | Meaning |
| --- | --- |
| `CHAT_S3_ACCESS_KEY` | Access key. Required when storage is enabled. |
| `CHAT_S3_SECRET_KEY` | Secret key. Required when storage is enabled. |
| `CHAT_S3_SESSION_TOKEN` | Optional temporary-credential token. |
| `CHAT_S3_ENDPOINT` | Overrides `s3.endpoint`. Setting it enables storage even without an `s3` block. |
| `CHAT_S3_PUBLIC_ENDPOINT` | Overrides `s3.public_endpoint`. |
| `CHAT_S3_BUCKET` | Overrides `s3.bucket`. |
| `CHAT_S3_REGION` | Overrides `s3.region`. |
| `CHAT_S3_PATH_STYLE` | `true` or `false`; overrides `s3.path_style`. |

Credentials are read only from the environment; instance and task roles and metadata
services are not used. `CHAT_S3_*` values override saved configuration for that process.
Media limits and retention (`media.max_size`, default 25 MiB; `media.max_age`, default 7
days; quotas) are separate from storage settings; see
[Media uploads](media-uploads.md).

For systemd, put `CHAT_S3_*` values in `/etc/chat.env`, not on a shell that only runs
`systemctl`.

### Bucket CORS

Browser uploads use presigned `PUT`, so the bucket must allow the UI origin. This is the
`PutBucketCors` rule set. Enter the same values in the R2 dashboard, or set the same rule
through the RustFS console or any S3 client:

```json
{
  "CORSRules": [
    {
      "AllowedOrigins": ["https://chat.example.com"],
      "AllowedMethods": ["PUT", "GET", "HEAD"],
      "AllowedHeaders": [
        "content-type",
        "x-amz-meta-client-filename",
        "x-amz-meta-client-content-type",
        "x-amz-meta-client-label",
        "x-amz-meta-client-client"
      ],
      "ExposeHeaders": ["ETag"],
      "MaxAgeSeconds": 300
    }
  ]
}
```

Use the exact scheme, host, and port of the UI; a wildcard origin is not appropriate for
a private bucket. Completion is server-side and does not need CORS. R2 can take up to 30
seconds to propagate CORS changes.

### Provider comparison

| | AWS S3 | Cloudflare R2 | RustFS |
| --- | --- | --- | --- |
| `endpoint` | `https://s3.<region>.amazonaws.com` | `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` | internal origin, e.g. `http://127.0.0.1:19000` |
| `public_endpoint` | `https://<bucket>.s3.<region>.amazonaws.com` | same as `endpoint` (presigned URLs cannot use custom domains) | public HTTPS origin, e.g. `https://media.example.com` |
| `region` | bucket region | `auto` | `us-east-1` unless configured |
| `path_style` | `false` | `true` | `true` |
| Credentials | IAM access key | R2 API token | `RUSTFS_ACCESS_KEY` / `RUSTFS_SECRET_KEY` |
| Lifecycle | S3 rules | prefix-based rules | where supported |

### Provider setup

#### AWS S3

1. Create a bucket in the target region with Block Public Access on and default
   encryption (SSE-S3) enabled. Do not require extra headers such as SSE-C; presigned
   URLs sign `content-type`, `content-length`, and accepted client hint headers.
2. Create an IAM principal for the server with this policy, scoped to the bucket:

   ```json
   {
     "Version": "2012-10-17",
     "Statement": [
       {
         "Effect": "Allow",
         "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
         "Resource": [
           "arn:aws:s3:::commonroom-media/uploads/*",
           "arn:aws:s3:::commonroom-media/media/*",
           "arn:aws:s3:::commonroom-media/avatars/*"
         ]
       }
     ]
   }
   ```

   `CopyObject` needs `GetObject` on the source and `PutObject` on the destination, both
   included above; `ListBucket` is not needed. A bucket policy that denies requests
   missing extra headers also breaks uploads.
3. Apply the CORS rule from [Bucket CORS](#bucket-cors).
4. Set `path_style` to `false`, `region` to the bucket region, and leave
   `public_endpoint` unset so it derives
   `https://<bucket>.s3.<region>.amazonaws.com`.
5. Export `CHAT_S3_ACCESS_KEY` and `CHAT_S3_SECRET_KEY` to the service.
6. Optionally add lifecycle rules for `uploads/` and `media/<N>d/` prefixes; see
   [Lifecycle rules](#lifecycle-rules).
7. Add `https://<bucket>.s3.<region>.amazonaws.com` to the UI CSP, then verify an
   upload.

#### Cloudflare R2

1. Create a private R2 bucket. Do not enable public access or an `r2.dev` domain.
2. Create an R2 API token with **Object Read & Write**, scoped to that bucket, and note
   the Access Key ID and Secret Access Key.
3. Configure:
   - `endpoint`: `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`
   - `public_endpoint`: the same value. Presigned URLs work only on the S3 API host;
     custom domains cannot be used for them.
   - `region`: `auto`
   - `path_style`: `true`
4. Apply the CORS policy from [Bucket CORS](#bucket-cors): UI origin, `PUT`, `GET`,
   `HEAD`, and `content-type` plus `x-amz-meta-client-*`.
5. Add lifecycle rules per prefix if desired; R2 applies them asynchronously. Rules are
   prefix- and day-based, and object tags are not supported (chat does not use them).
6. Add `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` to the UI CSP and verify an
   upload and download.
7. Do not put a shared cache or CDN in front of the signed URL path.

Switching providers later means new credentials, CORS, lifecycle rules, and object
copies; objects do not move themselves.

#### RustFS

RustFS is an S3-compatible server you host yourself; use it when media must stay on your
infrastructure. The repository's `auto/start-s3.sh` starts a development instance; the
same binary runs in production with different paths and credentials.

1. Install the `rustfs` binary (or container) and give it a dedicated data directory,
   for example `/srv/rustfs/data`, owned by an unprivileged service user.
2. Start it bound to loopback, with credentials from the environment:

   ```sh
   RUSTFS_ACCESS_KEY=... RUSTFS_SECRET_KEY=... \
     rustfs server --address 127.0.0.1:19000 \
     --console-address 127.0.0.1:19001 /srv/rustfs/data
   ```

   Keep the console private, or disable it (`RUSTFS_CONSOLE_ENABLE=false`) when it is
   not needed. Never expose either port directly to the internet.
3. Terminate TLS for browsers on a separate hostname, for example
   `https://media.example.com`, and proxy it to `127.0.0.1:19000` with the original
   `Host`, path, and query string intact; presigned URLs sign them. Allow request bodies
   at least as large as `media.max_size`:

   ```nginx
   server {
       listen 443 ssl;
       server_name media.example.com;

       location / {
           proxy_pass http://127.0.0.1:19000;
           proxy_set_header Host $host;
           client_max_body_size 30m;
           add_header X-Content-Type-Options nosniff always;
       }
   }
   ```

   Do not enable a shared proxy cache here, and do not attach chat cookies to this host.
4. Create the private bucket and apply the CORS rule from [Bucket
   CORS](#bucket-cors) through the RustFS console or any S3 client. The server does not
   create or configure production buckets; its `--setup-local-s3` helper only accepts
   `http://127.0.0.1` and path-style access.
5. Configure chat with the internal endpoint, the public endpoint, path style, and the
   same credentials:

   ```json
   {
     "s3": {
       "endpoint": "http://127.0.0.1:19000",
       "public_endpoint": "https://media.example.com",
       "bucket": "commonroom-media",
       "region": "us-east-1",
       "path_style": true
     }
   }
   ```

   Export `CHAT_S3_ACCESS_KEY` and `CHAT_S3_SECRET_KEY` with the values RustFS was
   started with.
6. Add `https://media.example.com` to the UI CSP.
7. Back up the RustFS data directory together with SQLite. Both must describe the same
   point in time: stop chat and RustFS, or snapshot the volume, before copying.

The internal HTTP endpoint is loopback-only and is not the URL browsers see; production
requires the public endpoint to be HTTPS.

### Lifecycle rules

Chat sweeps expired and unreferenced tracked objects at startup and every 24 hours
by default, with short retries for failed/incomplete sweeps. Saved
`media.cleanup_interval` overrides remain in effect. Serve `media-cache.js` from
the UI base path with JavaScript MIME and background revalidation
(`max-age=0, stale-while-revalidate=7200`, as in the supplied Nginx include);
the worker scope must cover that application's API paths. The UI
worker caches downloaded immutable attachment and managed-avatar bytes privately
and checks current `/info` authorization before reuse. Avatar redirects and signed
storage responses remain `no-store`; do not cache them at a shared proxy. External
URL avatars load only for viewers who enable them in Security (default off). Keep its `connect-src` policy able to reach storage.

Chat deletes expired and retracted objects itself with retries. A bucket
lifecycle rule is a backstop for objects that outlive their metadata because a process
died mid-cleanup; it is asynchronous and rounds to days. Rules key off the prefix, not
object tags (R2 has no tags).

The final key encodes the retention class:

| Prefix | Contents |
| --- | --- |
| `uploads/` | Staging objects; cleanup deletes them once they are older than the pending TTL (default 24h). |
| `media/1d/`, `media/7d/`, ... | Whole-day metadata lifetimes, `media/<N>d/`. |
| `media/<N>s/` | Sub-day lifetimes such as `media/3600s/`. |
| `avatars/` | Permanent account avatar files; do not apply an expiry lifecycle rule. |
| `media/permanent/` | Metadata with no expiry (`-1`). |

Reasonable rules: expire `uploads/` after 1 day, `media/1d/` after 1 day, and
`media/7d/` after 7 days. Add one rule per whole-day class you actually use, never
expire a class sooner than the grant period it represents, and leave
`media/permanent/` alone. On versioned buckets also expire noncurrent versions, or
objects never disappear. Use a dedicated prefix (or bucket) per deployment.

### Storage headers and CSP

The HTML document is served by Nginx, so its CSP must name the storage origin; the
server's CSP additions only cover API responses.

Edit the sample's `add_header Content-Security-Policy` lines in both general UI
and hashed-asset locations: append the public storage origin to both `connect-src 'self' wss://$http_host`
and `media-src 'self'`. `img-src` already allows `https:` and `blob:` for previews, so
images need no change. For AWS S3 the origin is
`https://<bucket>.s3.<region>.amazonaws.com`; for R2 it is
`https://<ACCOUNT_ID>.r2.cloudflarestorage.com`; for RustFS it is your
`public_endpoint`.

Storage responses should be private and uncached, and must not be served from the chat
origin. Keep `X-Content-Type-Options: nosniff` on them (the sample RustFS proxy sets it),
never add chat cookies, and do not put a public CDN in front of presigned URLs. Keep the
bucket private: downloads use short-lived signed URLs tied to current access, and
retracting a message stops new URLs.

### Verify uploads

After configuring any provider:

- Sign in as a user who can upload in a room (or receives a grant), send an image, and
  confirm it renders inline and downloads.
- Retract it and confirm the message disappears; the worker deletes the object on its
  next pass.
- Upload a file just under and just over `media.max_size`, and confirm the failure is
  the size limit, not a storage error.
- Watch the log: storage outages warn and fail uploads with `Upload rejected: media
  storage is unavailable. Try again later.` while the rest of the site keeps working.
- Restart once and confirm the same bucket and configuration are picked up.

For a local RustFS instance, `rtk proxy node web/scripts/media-smoke.mjs` exercises
signing, CORS, upload, download, retraction, and a non-root base path. It needs
`tmp/rustfs`. Provider credentials, CORS, ingress limits, and lifecycle rules still need
a real bucket check; the local suite does not run against AWS or R2.

### Backups

Stop chat (and RustFS, if self-hosted) before copying, or snapshot consistently. Back up
the SQLite database and the bucket together: a database restored without its objects has
broken messages, and objects restored without the database have no permissions or
references. Do not copy `data/` while the server is running.

## External agent workers

Agents use the bot HTTP/WebSocket API under `/api/bot/v1/`, which the existing `/api/`
location already proxies. Keep `Authorization` and `Idempotency-Key` headers intact, and
give each worker its own runtime directory, supervisor, secrets, and checkpoint store;
run one worker per checkpoint. Use a distinct bot account when its permissions differ
from other workers. The minimal worker and checkpoint contract are in
[Agent development](agent-development.md); scoped provisioning and retry guarantees are
in [Bots](bots.md).

## Upgrades and backups

SQLite remains HEAD schema v19 and opens without an upgrade. Unpublished
PostgreSQL initializes its current v1 schema, including statement-level deletion
retention. This unpublished format is not deployed yet; fixes modify v1 directly,
without historical WIP upgrades. Unsupported schema markers are rejected unchanged.
The offline transfer always maps HEAD SQLite to current PostgreSQL and leaves
the SQLite source unchanged. Back up the
database and object store consistently before replacing a deployment ([Data](data.md)).

Stop the old process before starting the new one; only one process may use a data
folder. Replace the binary and UI assets, then start the new version and run the
[post-deploy checks](#post-deploy-checks).

Hard ceilings remain 256 users, 1,024 rooms, and 1,000,000 retained messages; plan
storage and backups around them.

PostgreSQL defaults to 2–8 reader sessions, up to 8 writer sessions and one
dedicated ownership session. Tune `database.postgres_pool` to the host's connection
budget; see [PostgreSQL](postgresql.md#runtime-and-backups) for limits and an example.

## Post-deploy checks

- `http://` redirects to `https://`, and HSTS is present on the page and API responses.
- Login works, the session cookie is `Secure` and `HttpOnly`, and its path matches
  `base_url`.
- The UI loads assets and `markdown-frame.html` under the configured prefix; a message
  with Markdown renders without CSP errors.
- The WebSocket connects with `wss://` and reconnects after a server restart.
- If storage is enabled, an upload renders and downloads, and no file bytes pass through
  the chat origin.
- Configuration survives a restart: the values in `data/config.json` match the running
  setup.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Uploads fail with `Upload rejected: media storage is unavailable. Try again later.` | The server's `endpoint` is reachable, its TLS and DNS are valid, and the credentials can `GetObject` on `uploads/*`. The upload probe uses `uploads/.availability` and accepts 200, 403, or 404. |
| Browser reports a CORS error on the upload `PUT` | CORS `AllowedOrigins` matches the UI origin exactly, `PUT` is allowed, and `content-type` is in `AllowedHeaders`. On R2, allow up to 30 seconds for propagation. |
| `403 SignatureDoesNotMatch` on upload or download | A proxy rewrote the signed host, path, or query, or `public_endpoint` differs from the URL users open. Preserve `Host` and the raw request target. |
| Upload completion is rejected | The `PUT` did not finish or sent a different size; check that the storage proxy body limit is at least `media.max_size` (default 25 MiB) and that any provider ingress limit allows it. |
| Browser console reports a CSP violation for media | Add the public storage origin to `connect-src` and `media-src` in the Nginx asset location; backend headers do not apply to static files. |
| Logins are throttled for everyone at once | The server sees one client IP (a proxy or Cloudflare edge). Configure real-IP handling on Nginx and keep `trust` on the actual socket peer. The limits are 10 failures per IP and 20 per username per minute. Successful logins do not consume the allowance. Account failures also apply exponential backoff up to 32 seconds. |
| Login or API calls fail with a forwarded-header or origin error | `X-Forwarded-Proto` must be `https` (except direct bot calls from `trust`), and `origins` must match the browser origin without a path. |
| Assets or the WebSocket 404 after changing `base_url` | Update both the Nginx locations and `base_url`; they must agree, including the trailing slash. |

### Explicit direct bot HTTP

Production requires forwarded HTTPS for bot requests by default. For intentional
same-host/private bot access, set `direct_bot_http` to true or start with
`CHAT_DIRECT_BOT_HTTP=1` (`0` disables it). This requires a loopback/private `trust`
address and applies only to bearer requests under `/api/bot/v1/` when the protocol
header is absent. Explicit `X-Forwarded-Proto: http` is always rejected. Keep the
listener private and make the proxy overwrite `X-Forwarded-Proto` on every
request; enabling this mode is unsafe with a proxy that omits that header.

External avatar URLs are denied unless their exact hostname is listed in
`avatar_hosts`, for example `["www.gravatar.com"]`. Up to 32 canonical hosts are
accepted. Existing disallowed URLs remain stored but are hidden from snapshots;
managed image uploads do not need an external host allowance.

Deploy the rebuilt web bundle with backend changes that introduce compact avatar
profile frames. Existing open tabs obtain the new client when reloaded; normal
HTTP resynchronization continues to return the full current avatar directory.
