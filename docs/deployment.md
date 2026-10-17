# Deployment

## Build and run

To build manually:

```sh
rtk pnpm --dir web install
rtk proxy ./auto/check.sh
rtk pnpm --dir web build
rtk cargo build --release
rtk proxy cp target/release/chat ./chat
rtk proxy tar -cJf ui.tar.xz -C web dist
```

Builds can also use `rtk proxy ./auto/build.sh` (release by default). All shell
scripts live in `auto/` and work from any current directory. The build script
installs dependencies and builds without running source checks or tests; run
`rtk proxy ./auto/check.sh` separately. It generates missing lockfiles on its
first run and uses frozen/locked dependencies subsequently. Release builds
produce `./chat` and `./ui.tar.xz`; the archive contains the `dist/` folder from
`web/dist/`. Extract it with `tar -xJf ui.tar.xz -C /srv/commonroom` (create that
directory first), then point Nginx at `/srv/commonroom/dist/`. Node and pnpm are
not needed on the destination server. Deploy the binary and UI independently,
keeping their protocol versions compatible.

Rust builds have no frontend dependency: `rtk cargo build --release --locked`
works without web assets or Node. Debug builds (`./auto/build.sh debug`) produce
`target/debug/chat` and `web/dist-debug/`; serve that UI directory with Nginx or
use Vite at http://localhost:5173 during development. `auto/debug.sh` runs the
existing server binary and serves `web/dist-debug/` with Vite preview at
http://127.0.0.1:5173, proxying API/WebSocket requests to `127.0.0.1:3000`.
It preserves the stdin console and stops both processes on Ctrl + C or when
either exits. This assumes root hosting and the default backend port;
use Nginx for custom listener/base-path or production settings.
`auto/clean.sh` removes `target/`, `web/dist/`, `web/dist-debug/`, Vite caches,
`./chat`, and `./ui.tar.xz`, preserving `data/`, dependencies, and lockfiles.

First production start:

```sh
rtk proxy env CHAT_ORIGIN=https://chat.example.com ./chat
```

Put the server behind Nginx; [deploy/nginx.conf](../deploy/nginx.conf) is a
location-only include for `/commonroom/`. Place it inside your existing HTTPS
`server {}` block (under `http {}`). Nginx does not allow `location` directly
inside `http`. Your existing server supplies TLS certificates and HTTP-to-HTTPS
redirection. Match `base_url` to the location prefix. The example preserves
the prefix when forwarding; do not add a trailing slash to `proxy_pass`.
Nginx serves `/commonroom/` and its static files directly from the extracted
`dist/` folder. It proxies `/commonroom/api/` and the exact `/commonroom/ws`
endpoint to Rust. Unknown static files return 404. Keep the trailing-slash
redirect so relative asset and API URLs resolve correctly. For root hosting,
set `base_url` to `/`, change the locations to `/api/`, `/ws`, and `/`, and
remove the `/commonroom` redirect.
The UI renders Markdown images/GIFs using validated absolute HTTP/HTTPS or
base64 raster URLs, lazy loading and `no-referrer`. Keep `img-src` for those
images and `font-src 'self' data:` for KaTeX fonts that Vite may inline.
All HTML previews require a per-message Render HTML click and use
`markdown-frame.html`, generated into both UI builds. Deploy that file with the
assets. The exact Nginx location in the example must precede/generalize correctly
with your base path: it applies a separate CSP permitting inline scripts/styles
only inside the sandboxed preview, plus `frame-ancestors 'self'`. Do not apply the
main app's `frame-ancestors 'none'` / strict script policy to this file. Keep the
main app's `script-src 'self'`, `style-src 'self'`, `frame-src 'self'` and
`frame-ancestors 'none'`; do not add unsafe-inline to the main app.

Frame URLs carry a content version; `public, max-age=31536000, immutable` avoids
repeated network fetches when virtual rows remount. The shell has no external
bundle requests, needs no public CORS headers and receives message HTML only via
a one-shot postMessage, never a URL. Its leading meta CSP and iframe sandbox also
apply without Nginx; a main-document meta policy restricts frames to self.
Previews permit inline code and data media/fonts but deny network fetches, external
scripts/styles, forms, nested frames, popups, parent access and top navigation.
Frames are frontend static assets; no backend preview/proxy route is added.
Validate with `rtk proxy nginx -t` before reloading Nginx.

Release binaries require HTTPS, check request origins, set secure HttpOnly
cookies scoped to `base_url`, send HSTS and CSP on backend responses, and require
`X-Forwarded-Proto: https` except for direct bearer-authenticated bot requests from `trust` under `/api/bot/v1/`. Nginx supplies the security headers
for static UI responses. The frontend redirects HTTP to HTTPS and uses
`wss:` in production. The proxy must overwrite forwarded headers. Production
requests must arrive from the exact socket-peer IP configured in `trust`;
forwarded headers cannot override that check. Keep the backend listener private
and restrict remote proxy deployments to the configured source IP. The Rust
backend deliberately does not terminate TLS.

The proxy upgrade headers follow the [official Nginx WebSocket guidance](https://nginx.org/en/docs/http/websocket.html).

Settings are saved as key-value pairs in `data/config.json`. Edit this file
while the service is stopped, then restart to apply changes. Example for Nginx
on the same machine:

```json
{
  "production": true,
  "bind": "127.0.0.1:3000",
  "trust": "127.0.0.1",
  "set_real_ip_from": "X-Forwarded-For",
  "base_url": "/commonroom/",
  "max_users": 64,
  "max_rooms": 64,
  "max_messages": 1000,
  "origins": ["https://domain.com"]
}
```

`bind` is the backend HTTP listener: it serves `/api/` endpoints and WebSockets
at `/ws` on the same port (3000 by default), with no static UI routes.
`base_url` prefixes backend paths at runtime: `/commonroom/api/login` and
`/commonroom/ws`. Serve the UI under the same prefix through Nginx; relative
URLs let the same UI build work at `/` or `/commonroom/`. `base_url` defaults
to `/` and accepts path segments containing letters, digits, `-`, and `_`; a trailing
slash is added automatically. It is a path, not a full URL. Nginx
exposes HTTPS and WSS on port 443, serves UI files, and forwards API/WebSocket
requests to this listener.
`trust` is a single proxy IP address, defaulting to `127.0.0.1`; use the load
balancer's actual source IP for a remote proxy, and bind to a reachable private
address. Trust is enforced in production. Existing config files without
`trust` receive the default automatically. Release builds always enforce
production mode; debug builds use the saved `production` setting.
`set_real_ip_from` names the header used for the client IP in login throttling.
It defaults to `"X-Forwarded-For"`, including for existing configuration files,
matching the example's `proxy_set_header X-Forwarded-For $remote_addr`.
Only the socket-peer IP configured in `trust` may supply this header, in either
production or development. The value must contain exactly one IPv4 or IPv6
address; missing, malformed, duplicate, or comma-separated values fall back to
the socket-peer IP. IPv4-mapped IPv6 addresses share the same throttle bucket
as their IPv4 form. Set it to `null` to disable header-based client IPs, or use
another header name such as `"X-Real-IP"` and configure Nginx to overwrite it.
This setting names a header, unlike Nginx's `set_real_ip_from` directive, which
names trusted proxy addresses. Changes require a restart.

Nginx must overwrite the chosen header with a verified address. Do not use
`$proxy_add_x_forwarded_for`: this server deliberately rejects address chains.
If Nginx sits behind Cloudflare or another proxy, `$remote_addr` identifies that
proxy unless Nginx's real-IP module is configured to trust that upstream and
resolve the actual client. Only trust your actual upstream proxies.
Login allows 10 attempts per client IP per minute; users sharing an IP still
share that budget.

`max_users` and `max_rooms` are positive integers, each defaulting to 64.
They limit new account and room creation through both web commands and stdin.
Disabled accounts count toward `max_users`. Lowering a limit preserves existing
accounts and rooms while preventing new creation until the count is below it.
`max_messages` is a positive integer, defaulting to 1000 for each room and each
private conversation. Each new message evicts the oldest when that conversation
is full, keeping the newest messages. A busy private conversation does not
evict another person's history. Lowering this limit trims and saves both room
and private histories at startup. v7 keeps retention in SQLite separately from the fixed memory cache budgets. Message sends use targeted transactions; retention is pruned through the indexed conversation/sequence key. Cache pages are created lazily, with at most 1,000 entries per conversation and 10,000/32 MiB accounted globally. Raising `max_messages` increases disk retention, not those memory budgets. Read positions use a separate
SQLite table with indexed, monotonic updates and do not rewrite message data.
Restart after editing these values.

```json
{
  "production": true,
  "bind": "127.0.0.1:3000",
  "trust": "127.0.0.1",
  "set_real_ip_from": "X-Forwarded-For",
  "base_url": "/",
  "origins": ["https://chat.example.com"],
  "max_users": 64,
  "max_rooms": 64,
  "max_messages": 1000
}
```

Set the public hostname to your actual domain. Enable HTTPS redirects at
Cloudflare (Always Use HTTPS) and keep WebSockets enabled. The backend checks
`X-Forwarded-Proto: https`, which Nginx supplies on its proxied requests.
Set backend `trust` to Nginx's actual socket-peer IP, and keep the Rust listener
private. The tunnel origin uses HTTPS so Nginx supplies the correct scheme.

Cloudflare Tunnel preserves request paths. For `/commonroom/`, set that
`base_url` and forward the path unchanged. Do not cache `/api/*` or `/ws`
(or their base-path equivalents) using custom Cloudflare cache rules.
The app reconnects if a tunnel restart interrupts an existing WebSocket.

Protocol compatibility is verified with production HTTP/WebSocket checks;
a real public tunnel still needs an end-to-end check on your own hostname.
See Cloudflare's [routing documentation](https://developers.cloudflare.com/tunnel/concepts/routing/),
[WebSocket support](https://developers.cloudflare.com/cloudflare-one/faq/cloudflare-tunnels-faq/),
and [forwarded headers](https://developers.cloudflare.com/fundamentals/reference/http-headers/).

`origins` checks the browser Origin header for login, logout, read acknowledgements, and WebSockets.
It contains scheme + host + optional port, never a path; for
`https://domain.com/commonroom/`, the origin is `https://domain.com`.
Production uses one HTTPS origin. `trust` checks the proxy IP separately.

Subsequent starts need only:

```sh
rtk proxy ./chat
```

Environment settings:

| Variable | Purpose |
| --- | --- |
| `CHAT_DATA` | Data-folder location; defaults to `./data` relative to the working directory |
| `CHAT_ORIGIN` | Public HTTPS origin, no path or trailing slash; required on first production start and saved |
| `CHAT_BIND` | Listener; defaults to `127.0.0.1:3000`, saved when overridden |
| `CHAT_TRUST` | Trusted proxy IP; defaults to `127.0.0.1`, saved when overridden |
| `CHAT_BASE_URL` | Hosting path; defaults to `/`, saved when overridden |
| `CHAT_PRODUCTION=1` | Exercise production security in a debug binary |
| `RUST_LOG` | Log filter; defaults to `chat=info,tower_http=info` |

Changing `CHAT_BIND`, `CHAT_TRUST`, `CHAT_BASE_URL`, or `CHAT_ORIGIN` persists the change.
Environment overrides take precedence over saved values. Production settings
cannot be silently downgraded by running a debug binary. A release binary
always enforces production security, including when upgrading a development
data folder; provide `CHAT_ORIGIN` the first time you upgrade it.

Logs go to stdout. To redirect them while preserving the stdin console:

```sh
rtk proxy ./chat > chat.log 2>&1
```

Closing stdin leaves the web service running. Ctrl + C or SIGTERM stops it
gracefully and checkpoints SQLite. No default account or password is shipped.

Direct bot access: a bot running on the configured `trust` socket IP may use HTTP/WS directly against `/api/bot/v1/` (including the configured base path) without `X-Forwarded-Proto`. Bearer authentication and current account grants remain required. Socket identity, not X-Forwarded-For, authorizes this exception. Browser endpoints still require forwarded HTTPS; a bot request carrying X-Forwarded-Proto must also indicate https. With the default bind/trust of 127.0.0.1, same-machine agents can connect directly; when trust names a remote proxy, a local process must use the proxy or originate from the configured trusted IP.

## Deploying an external agent

Use `/api/bot/v1/` for agent integrations; the existing `/api/` Nginx location
already proxies these HTTP and WebSocket routes, including the configured base
path. Keep `Authorization` and `Idempotency-Key` headers intact. The external
worker needs its own runtime/dependencies, process supervisor, secret injection
and private checkpoint directory; the chat server does not start it. Run one
worker per checkpoint and use a distinct bot account when permissions differ.
See [Agent development](agent-development.md) for the runnable reference worker.

This release accepts existing v12 data, transactionally upgrades validated v9/v10/v11
data, or initializes an empty data folder. Back up v9/v10/v11 data before upgrading;
older schemas remain unsupported. See [Data](data.md) for backups and compatibility.

Supported configuration ceilings are 256 users, 1,024 rooms and 1,000,000 retained
messages per conversation. Values beyond these ceilings are rejected before
configuration writes or engine retention changes; existing data is preserved.
