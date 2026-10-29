# Frontend development

The Preact client keeps network and feature state in focused hooks, with pure
conversation helpers under `src/features/conversation/`. The server remains the
owner of permissions and persisted history. See the repository's `docs/` guides
for the wire contract and deployment setup.

## State ownership

- `useHistory` owns paging, cache selection, and cancellation. Cancelling a page
  clears loading even when the next conversation needs no HTTP request.
- `useReadTracking` coalesces visibility work while a read is in flight, then
  checks the latest visible conversation after completion. Sent positions use
  stable conversation IDs and retain at most 32 entries. Unmount cancels timers.
- `useCommands` coordinates composer submission. `useCommandRequests` owns its
  shared request lifecycle and account operations; `useOutgoingMessages` owns
  pending/failed messages; `useMessageActions` owns edit/reaction/retraction
  submission; `useCommandPreferences` owns aliases/Markdown preferences;
  `useCommandNavigation` owns acknowledgement-driven navigation/debug state;
  `useDeleteConfirmation` owns confirmation state.
- `components/settings/` contains appearance, formatting, security, and account
  editors. Account editors invalidate late results when unmounted. Avatar
  settings stay mounted while switching panels so an upload can finish.
- `MediaCache` authorizes each caller before consulting stored bytes. Concurrent
  full-object misses share a fill, with at most four fills and 256 MiB of reserved
  response copies waiting for cache writes per worker. Excess work streams from
  the network without a cache copy. This bounds cache work, not total browser
  memory. Persistent limits remain 128 entries/256 MiB. Cancellation of a waiter
  does not cancel another caller's fill; failures release admission reservations.

## Frontend checks

Run from the repository root:

```sh
rtk pnpm --dir web check
rtk pnpm --dir web test
rtk pnpm --dir web test:browser
rtk pnpm --dir web build
```

Unit hook tests share `tests/helpers/hooks.js` for deterministic state/effect
scheduling. Use this for request races and state invariants, not browser layout
or actual Preact commit ordering.

`test:browser` starts Vite on an ephemeral loopback port and launches an installed
Chrome/Chromium with a temporary profile. Set `CHROME_BIN` if automatic discovery
cannot find the executable. It requires permission to bind localhost and launch
Chrome. It never starts the backend or reads account data. The suite covers real
Preact conversation switching/cancellation, delayed read acknowledgements,
command confirmation/draft recovery, and settings-panel credential cleanup.
The browser fixture is also available at `/scripts/regressions/index.html` when
running the Vite development server.

## Tooling and build artifacts

`check` runs TypeScript, application/hook and JavaScript harness linting, CSS
checks, unused-export checks and formatting. TypeScript 7 performs typechecking;
TypeScript 6 supplies the compiler API used by type-aware ESLint. Keep both.

`dist/` is the production artifact. `dist-debug/` is the ignored development-mode
bundle used by `auto/debug.sh`; it is intentionally separate from release assets.
Neither directory is source. Test pages under `scripts/` are not build entries.
The browser suite launches an isolated Chrome profile and uses no real chat data.

## Hosting and message content

Serve production over HTTPS and configure redirects/HSTS at the reverse proxy;
the client does not infer an HTTPS port from the HTTP listener. The production
HTML includes a fallback CSP and no-referrer policy. The CSP permits HTTPS storage
connections and media, while scripts and workers remain restricted to this
origin. Keep deployment headers for exact storage-origin restrictions and
`frame-ancestors`, which cannot be enforced through a meta policy. Development
keeps the Vite HMR bootstrap separate from the production CSP. Debug artifacts
built with `--mode development` also permit HTTP storage for local RustFS.

External Markdown images require a click identifying their host; changing an
image URL requires consent again. Managed attachments and embedded raster images
load normally. HTML previews remain opt-in and sandboxed. Render only trusted
content: scripts can consume CPU or navigate within the preview, and an isolated
preview can still display misleading content.

Cached media bytes are reauthorized on every request, including seeks. Recency
is recorded separately from immutable file bodies, so playback does not rewrite
whole cached videos. Authorization is never cached or bypassed for offline use.

Styles are ordered modules in `src/styles/`, imported by `src/style.css`.
Preserve import order when changing the cascade; theme tokens live in `base.css`.
