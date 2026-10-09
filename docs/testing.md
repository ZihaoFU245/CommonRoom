# Testing

## Required checks

From the repository root:

```sh
rtk cargo fmt --all --check
rtk cargo test --locked
rtk cargo clippy --all-targets --locked -- -D warnings
rtk pnpm --dir web typecheck
rtk pnpm --dir web format:check
rtk pnpm --dir web test
rtk proxy ./build.sh debug
rtk proxy ./build.sh release
rtk pnpm --dir web test:integration
```

The web build runs TypeScript checks before Vite. Vite's own transpilation is not a substitute for type checking. Frozen pnpm and Cargo lockfiles make build dependencies reproducible. The scripts contain ordinary commands; RTK wraps their invocation only.

For formatting:

```sh
rtk cargo fmt --all
rtk pnpm --dir web format
```

## Coverage

Rust tests cover configuration validation, proxy trust, account permissions and deletion, room membership, private visibility, password/session behavior, retained history, reactions/replies/mentions, migrations, restart/move persistence, and failed-write rollback.

Web helper tests cover local command ordering, redaction, suggestions, Unicode mentions, notifications, browser timezones and daylight saving, fonts/storage failures, unread visibility, retention, and resynchronization races. Protocol/client tests exercise malformed nested JSON, frame variants, base-path URLs, credentials, abort signals, and API errors.

`web/scripts/smoke.mjs` starts isolated debug/release processes on ephemeral ports and uses HTTP and real WebSockets. Every received frame passes the same runtime validator as the application. It checks authorization, commands, privacy, second-device directories, bounded room/private history, read cursors, account deletion, migration/restart, embedded assets, base paths, trusted proxy/TLS headers, origin rejection, secure cookies, and logout disconnection. It never uses the working `data/` folder. Build both binaries first.

## UI verification

After changes to components/hooks, inspect both a wide desktop and compact view. Verify login, sidebar collapse and scrolling, room/private selection, local commands and hints, composing Unicode text, replies/reactions, settings, unread badges/jump, and reconnect behavior. Refresh must erase command output while restoring server chat history. Check browser errors and HTTP/WebSocket failures.

For production deployment, additionally verify the actual public hostname through Nginx/cloudflared: HTTPS redirects, WSS, session cookies, and the configured base path. Local forwarded-header checks do not exercise public DNS/TLS infrastructure. Validate Nginx configuration on the target host.
