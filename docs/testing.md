# Testing

## Required checks

From the repository root:

```sh
rtk proxy ./auto/check.sh
```

This runs Rust formatting, Clippy with warnings as errors, Rust tests, TypeScript
checks, type-aware ESLint, web formatting, and web tests. Dependencies must be
installed first. To run individual checks:

```sh
rtk cargo fmt --all --check
rtk cargo test --locked
rtk cargo clippy --all-targets --locked -- -D warnings
rtk pnpm --dir web typecheck
rtk pnpm --dir web lint
rtk pnpm --dir web format:check
rtk pnpm --dir web test
rtk proxy ./auto/build.sh debug
rtk proxy ./auto/build.sh release
rtk pnpm --dir web test:integration
```

`auto/build.sh` installs dependencies, bundles the UI, and compiles Rust without
running source checks or tests in either mode. Release builds package `./chat`
and `./ui.tar.xz` containing `dist/`. Run `auto/check.sh` separately.
`auto/debug.sh` runs the existing debug binary and previews `web/dist-debug/`
at http://127.0.0.1:5173 with API/WebSocket proxying; it does not rebuild.
`auto/clean.sh` removes server/UI build artifacts and Vite caches, preserving
data, dependencies, and lockfiles.

The standalone `pnpm --dir web build` runs TypeScript, ESLint, and formatting
checks before Vite. Vite's transpilation is not a substitute for type checking.
Frozen pnpm and Cargo lockfiles make dependencies reproducible. The scripts contain
ordinary commands; RTK wraps their invocation only.

For formatting:

```sh
rtk cargo fmt --all
rtk pnpm --dir web format
```

## Coverage

Rust tests cover configuration validation, proxy trust, verified client-IP login throttling and spoofed/malformed forwarding headers, account permissions and deletion, agent roles and the minimal access an agent account holds, agent triggers, ownership through policy grants, renames, provider resolution, web-search configuration, credential redaction, provider reply parsing, search result formatting, prompts, room membership, private visibility, password/session behavior, retained history, reactions/replies/mentions, schema rejection, restart/move persistence, and failed-write rollback.

Web helper tests cover local command ordering, redaction, suggestions, Unicode mentions, link tokenizing, notifications, browser timezones and daylight saving, fonts/storage failures, unread visibility, retention, and resynchronization races. Protocol/client tests exercise malformed nested JSON, frame variants, base-path URLs, credentials, abort signals, and API errors.

Build tests verify that both modes skip checks and that release packaging
contains the `dist/` directory with its files. Rust tests also verify that a poisoned engine
returns an unavailable response and never reuses potentially partial state.

`web/scripts/smoke.mjs` starts isolated debug/release processes on ephemeral ports and uses HTTP and real WebSockets. Every received frame passes the same runtime validator as the application. It checks authorization, commands, agent roles and credential redaction, agent provider and personality settings, privacy, second-device directories, bounded room/private history, read cursors, account deletion, schema rejection, restart, API-only routing, base paths, trusted proxy/TLS headers, origin rejection, secure cookies, and logout disconnection. It never uses the working `data/` folder. Build both binaries first.

`web/scripts/agent-check.mjs` drives one throwaway server through the agent
lifecycle: `/agent` creation, `/add` invitations, mention and auto replies in
rooms and private conversations, renames, permission boundaries, disabling,
removal, web-search configuration, restart persistence, and credential handling.
It always exercises the failure paths with unusable keys, and adds real provider
and search round trips when the keys below are set:

```sh
rtk pnpm --dir web test:agents
rtk proxy env CHAT_LIVE_AGENT_KEY=sk-... CHAT_LIVE_SEARCH_KEY=tvly-dev-... \
    pnpm --dir web test:agents
```

`web/scripts/provider-check.mjs` proves the provider selection: it starts one
throwaway server, moves an agent to another provider and model, and checks the
stored settings, the reported summary, the request destination, and that no
credential is exposed. The unreachable-base-URL case asserts the failure message
rather than contacting the address, and one request to a real provider host with
a deliberately invalid key asserts the rejection path, so the check needs
outbound HTTPS.

A live check spends a small amount of credit on the configured provider account
and a search credit on the configured search account. With a live search key it
also asserts that a question about current releases is answered with linked
sources and that a question needing no search is answered without them.
Both scripts need `target/debug/chat`; the release smoke phase also needs `./chat`.

The trust-model regressions additionally cover su group delegation and revocation,
protected su accounts (including disabled targets), creator ownership,
invitation-only rooms, per-chat command gates, read-only grants, cleanup age
constraints, cross-target authorization, private inspection, stable IDs,
ownership transfer, rename/cursor preservation,
owner delegation and revocation of invitations without enabling other management
permissions, loss of that delegation authority after ownership transfer,
and compiled-policy rollback. Every registered command, including every agent
command, must appear in the denial probe, so a new command cannot ship without a
test that an account without its grants is refused. Output regressions verify that
@global and its legacy
alias resolve to the same scope, command requirements do not grant actions,
and assignment privacy and conditional cleanup limits survive formatting. The room
directory excludes other users' private pairs even with explicit inspection grants
or su membership. `/console` checks scoped grants, rejects extra arguments, and
does not change authorization or persisted state. Run
the manual optimized permission benchmark with:

```sh
rtk cargo test --release --locked permission_lookup_benchmark -- --ignored --nocapture
```

## UI verification

After changes to components/hooks, inspect both a wide desktop and compact view. Verify login, sidebar collapse and scrolling, room/private selection, local commands and hints, composing Unicode text, replies/reactions, settings, unread badges/jump, and reconnect behavior. Refresh must erase command output while restoring server chat history. Check browser errors and HTTP/WebSocket failures.

For production deployment, additionally verify the actual public hostname through Nginx/cloudflared: HTTPS redirects, WSS, session cookies, and the configured base path. Local forwarded-header checks do not exercise public DNS/TLS infrastructure. Validate Nginx configuration on the target host.

## Authorization coverage matrix

The trust suite checks every declared action against fresh user/admin/su/console
principals across server, owned room, foreign/private, account and unknown-resource
scopes. An explicit probe table covers denial of every registered command from
Command, room and private contexts for an account with no grants; adding a command
without a probe fails the test. HTTP history/read operations and snapshot payloads
are tested independently of command invocation.

Boundary tests cover direct versus group syntax, additive revoke behavior, scopes
named after groups, malformed requests, su-only delegation and protected accounts,
partial-authority invitations/group assignment, cross-target command borrowing,
message ownership and identity reuse, private visibility, read-only controls,
cleanup constraints, persistence failures and v5 restart and rejection of unsupported schemas.

A deterministic password-job test queues hashing behind a blocking worker, then
revokes the action or command, promotes the target to su (active or disabled),
replaces the target and regrants its privileges, revokes the session, disables
the actor, or revokes account-creation authority. Each case verifies the specific rejection and that
no stale operation modifies the resulting state. This test uses no timing sleeps.

These are behavioral and boundary checks, not a claim of 100% line/branch coverage
or a proof that every possible attack is excluded. Update this matrix and add
positive/negative cases whenever new permissions, commands or resource types are
introduced.

Sudo regressions cover the two independent global grants, insufficient room
grants, temporary authority on success/rejection/storage failure, caller identity
in ownership/messages/audit, self-revocation, and denied nested commands. Password
jobs additionally recheck both sudo grants, actor session/disabled status, and
target identity after hashing. Frontend checks mask wrapped passwords and retain
wrapped command completion and routing.
