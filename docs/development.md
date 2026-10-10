# Development

## Setup

Use Rust 1.89+ (edition 2024), Node 22.18+ with native TypeScript stripping, pnpm, and RTK. Install the web dependencies from the committed lockfile:

```sh
rtk pnpm --dir web install --frozen-lockfile
rtk cargo run --locked
```

Create accounts in the server's stdin console:

```text
/user alice alice admin
/user bob bob user
/new demo
/add alice demo
/add bob demo
```

Run the web development server in another terminal:

```sh
rtk pnpm --dir web dev
```

Open http://localhost:5173. Vite proxies `/api` and `/ws` to the Rust listener at `127.0.0.1:3000`. Development uses plain WS; release assets use HTTPS/WSS. The default Vite proxy assumes root hosting. To verify a configured base path, serve the built UI with Nginx or adjust the development proxy.

To build the debug server and UI:

```sh
rtk proxy ./auto/build.sh debug
rtk proxy ./auto/debug.sh
```

Serve `web/dist-debug/` with Nginx, or use Vite at http://localhost:5173. `debug.sh` runs the existing artifact; it does not rebuild. Rebuild the relevant artifact after changing web assets or Rust code. Release
packaging uses `rtk proxy ./auto/build.sh release` and produces `./chat` and
`./ui.tar.xz` (containing `dist/`).

All shell scripts live in `auto/` and locate the repository root themselves.
Both build modes skip source checks and tests. Run `rtk proxy ./auto/check.sh`
separately before deploying.

Set `CHAT_DATA` to an isolated directory for experiments. Existing `data/` contains real account credentials, sessions, and private history; do not delete or modify it for tests. Tests create their own temporary folders. Stop a server before copying its entire data folder.

## Coding conventions

- Read [Architecture](architecture.md) before changing state ownership or module boundaries.
- Prefix commands you execute with RTK. Use `rtk proxy` for unsupported commands. Build/run scripts themselves contain ordinary commands; do not insert RTK into their bodies.
- Write application web code in `.ts`/`.tsx`. Keep strict null/index/optional-property checks, explicit return paths, switch fallthrough checks, and unused/unreachable-code checks enabled. Use `unknown` at JSON boundaries, validate it, then narrow to a protocol type. Avoid `any`, non-null assertions, unchecked protocol casts, `@ts-ignore`, and disabling checks to make a build pass.
- `pnpm --dir web typecheck` explicitly uses TypeScript 7 via the `@typescript/native` alias. The separate `typescript` 6 dependency supplies the supported compiler API for type-aware ESLint. Keep both lockfile versions compatible with their tooling; the default bare `tsc` executable belongs to the API dependency. ESLint rejects unsafe assignments/returns, unhandled promises, asynchronous callbacks used as void handlers, and non-null assertions. Handle promises with `await` or an explicit rejection handler; `void promise` does not bypass the rule.
- Declare component props and type DOM refs/events. Keep rendering components separate from connection, history, and command state. Prefer pure feature helpers and focused hooks.
- Preserve current UI conventions: the light theme keeps its paper background and dark
  normal text, both themes keep orange primary buttons and unread badges, and chat stays
  left-aligned with browser-local dates and Unicode message text. Themed surfaces read
  the palette custom properties at the top of `style.css`; add a new color to `:root`
  and to `:root[data-theme="dark"]` rather than hardcoding it in a rule, then add the
  same token to `paletteTokens` and derive it in `features/preferences/palette.ts`. A
  custom palette writes every entry of that list as an inline property, so a token
  missing from it silently keeps the built-in value. Text drawn on `--accent` must use
  `--accent-label` or `--accent-contrast`, never `--ink`, because the accent fill does not
  change between themes.
- Keep business permissions in the engine. Browser visibility/completion is not authorization. Keep SQL in `engine/storage.rs`, and avoid holding the engine mutex across `.await`.
- Cargo manifests forbid application unsafe code and ignored must-use values. Clippy rejects narrowing/sign-changing casts, `unwrap`/`expect`, explicit panic/unreachable/todo/debug macros. Return errors for invalid state and conversions. `unwrap`/`expect` are allowed only in test modules; deliberately injected panics need a local documented test allowance. Never recover a poisoned engine with `into_inner`; return unavailable and require a restart.
- Preserve password masking, session revocation, per-conversation retention, monotonically advancing read cursors, and tab-local command output. Never log credentials or message bodies.
- Keep migrations compatible with existing `data/`. Any schema change needs a version increment, migration tests, rollback behavior, and an update to [Data](data.md).
- Use `cargo fmt` and the web Prettier scripts. Update the relevant guides when paths, commands, protocol, configuration, or deployment behavior changes.

## Adding a feature

1. Put its persisted/wire model in `engine/models.rs` when needed.
2. Implement domain behavior in its owning engine module; route commands in `dispatch.rs` and register help/completion metadata in `server/src/commands.rs`.
3. Update `web/src/api/protocol.ts` types and runtime validators together with the Rust wire contract. Add/update contract checks in the integration harness.
4. Implement presentation in a component and stateful behavior in a focused hook; reuse the pure feature helpers.
5. Add tests for permissions, persistence failures, privacy, or synchronization risks introduced by the change. Run the checks in [Testing](testing.md).

The JavaScript files in `web/tests/` and `web/scripts/` are runtime test harnesses. Node loads the application's TypeScript helpers directly; fixtures intentionally include malformed/partial input. Production application source and Vite configuration are TypeScript and are checked before every web build.
