# Deployment

## Build and run

To build manually:

```sh
rtk pnpm --dir web install
rtk proxy ./auto/check.sh
rtk pnpm --dir web build
rtk cargo build --release
rtk proxy cp target/release/chat ./chat
```

Builds can also use `rtk proxy ./auto/build.sh`. Release builds first run the full
source check/test suite from `auto/check.sh`; failure stops bundling and packaging.
Debug builds (`./auto/build.sh debug`) skip that suite. All shell scripts live in
`auto/` and work from any current directory. The script generates
missing lockfiles on its first run and uses frozen/locked dependencies on
subsequent runs. Both lockfiles are included for reproducible builds. The script
itself uses ordinary commands; RTK only wraps your invocation. Release compilation requires a built
frontend. Assets are embedded into `chat`, so neither `web/` nor Node
is needed on the destination server. A debug binary embeds assets if they
exist at compile time; normally development uses Vite instead.

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
For root hosting, set `base_url` to `/` and use a single `location /` proxy.
Validate with `rtk proxy nginx -t` before reloading Nginx.

Release binaries require HTTPS, check request origins, set secure HttpOnly
cookies scoped to `base_url`, send HSTS and CSP, and reject requests without
`X-Forwarded-Proto: https`. The frontend redirects HTTP to HTTPS and uses
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
  "base_url": "/commonroom/",
  "max_users": 64,
  "max_rooms": 64,
  "max_messages": 1000,
  "origins": ["https://domain.com"]
}
```

`bind` is the single HTTP listener: it serves the embedded frontend, `/api/`
endpoints, and WebSockets at `/ws` on the same port (3000 by default).
`base_url` prefixes all these paths at runtime without rebuilding the frontend:
`/commonroom/`, `/commonroom/api/login`, `/commonroom/ws`. It defaults to `/`
and accepts path segments containing letters, digits, `-`, and `_`; a trailing
slash is added automatically. It is a path, not a full URL. Nginx
exposes HTTPS and WSS on port 443 and forwards both to this listener.
`trust` is a single proxy IP address, defaulting to `127.0.0.1`; use the load
balancer's actual source IP for a remote proxy, and bind to a reachable private
address. Trust is enforced in production. Existing config files without
`trust` receive the default automatically. Release builds always enforce
production mode; debug builds use the saved `production` setting.
`max_users` and `max_rooms` are positive integers, each defaulting to 64.
They limit new account and room creation through both web commands and stdin.
Disabled accounts count toward `max_users`. Lowering a limit preserves existing
accounts and rooms while preventing new creation until the count is below it.
`max_messages` is a positive integer, defaulting to 1000 for each room and each
private conversation. Each new message evicts the oldest when that conversation
is full, keeping the newest messages. A busy private conversation does not
evict another person's history. Lowering this limit trims and saves both room
and private histories at startup. Queue append/eviction is O(1); message saves
still serialize the retained application state. Read positions use a separate
SQLite table with indexed, monotonic updates and do not rewrite message data.
Restart after editing these values.

### Cloudflare Tunnel (without Nginx)

Cloudflare Tunnel can connect directly to this HTTP server and supports the
same WebSocket endpoint. For a named tunnel, map your public hostname to
`http://127.0.0.1:3000`. Use [deploy/cloudflared.yml](../deploy/cloudflared.yml)
for a locally managed tunnel, or enter that service URL in the Cloudflare dashboard.
Run `cloudflared` on the same machine with:

```json
{
  "production": true,
  "bind": "127.0.0.1:3000",
  "trust": "127.0.0.1",
  "base_url": "/",
  "origins": ["https://chat.example.com"],
  "max_users": 64,
  "max_rooms": 64,
  "max_messages": 1000
}
```

Set the public hostname to your actual domain. Enable HTTPS redirects at
Cloudflare (Always Use HTTPS) and keep WebSockets enabled. The backend checks
`X-Forwarded-Proto: https`, which Cloudflare supplies for HTTPS visitors;
its own connection from `cloudflared` remains HTTP. No separate WebSocket route
or TLS certificate is needed on this server. Use `127.0.0.1` rather than
`localhost` in the service URL so the socket peer matches `trust`. If your
connector is on another machine or in a container, adjust `bind` and `trust`
to the reachable address and actual connector source IP.

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
