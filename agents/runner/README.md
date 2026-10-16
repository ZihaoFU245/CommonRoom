# Agent runner

One Rust process, one V8 isolate per agent. The runner is a chat bot: it owns the
models, the tools and the isolates, and every agent is a directory under `data/`.

This replaces the earlier Deno-supervisor design (one `deno run` OS process per
agent). There is no backward compatibility with it.

## Status

Implemented and verified in WSL (21 tests, clippy clean under `server/`'s lint
set):

| Piece | State |
| --- | --- |
| `deno_core` 0.412 / V8 150.4 embedded, isolate starts | verified by `agents/auto/probe-deno-core.sh` |
| Configuration: `data/config.json`, created with defaults, namespaced under `agents` | done |
| Workspaces: `data/workspace<Name>/` with descriptor, credential, permissions | done |
| CLI: `serve`, `list`, `config` | `list` and `config` done; `serve` not wired yet |
| Guest-host bridge: `#[op2]` ops, isolate pool | not written |
| Chat client: Bot API v1 (snapshot, cursors, outbox, commands, socket) | not written |
| Control commands: `/status`, `/ps`, `/set <agent> token <value>` | not written |
| stdin console | not written |
| rig guest rebuilt for this interface | not written |

`serve` exits with a clear message rather than pretending to run.

## Layout

```text
agents/
  Cargo.toml              runner workspace (independent of the server workspace)
  runner/                 the Rust runner
    src/lib.rs            library: config, errors, workspace
    src/main.rs           CLI binary
    src/cli.rs            argument parsing
    src/config.rs         data/config.json, defaults, owner-only writes
    src/errors.rs         one error enum for the crate
    src/workspace.rs      one directory per agent, path confinement
  auto/                   POSIX sh helpers, WSL-first
    build.sh              stage to Linux fs, build, optionally build the guest
    check.sh              fmt + clippy + test
    probe-deno-core.sh    toolchain and deno_core gate
    probe-toolchain.sh    what a V8 build needs
    probe-network.sh      registry reachability
```

## Data layout

```text
data/config.json                  shared with the chat server; the runner owns `agents`
data/workspaces/workspace<Name>/
    agent.json                    bot identity, module, enabled flag, settings
    agent.token                   this agent's chat credential, mode 0600
    state.json                    cursors, outbox, content revisions
    agent.log                     host log for this isolate
    files/                        the only directory the agent's tools may write
    agent.js                      default module, written on first create
```

`data/config.json` is shared with the chat server. The runner reads and writes
only its `agents` key, so server settings (`bind`, `origins`, …) survive
untouched. Point it elsewhere with `--config` or `AGENTS_CONFIG`.

## Build and check

Requires WSL (or Linux) with `clang`, `libc++-dev`, `libc++abi-dev`, `cmake`,
`ninja-build` and `pkg-config` for `rusty_v8`:

```sh
sudo apt-get install -y clang libc++-dev libc++abi-dev cmake ninja-build pkg-config
```

```sh
agents/auto/probe-deno-core.sh    # gate: does deno_core build and run here?
agents/auto/check.sh              # fmt, clippy with -D warnings, tests
agents/auto/build.sh debug        # runner binary
```

`check.sh` and `build.sh` copy the tree to a Linux filesystem first: cargo cannot
link reliably on `/mnt/d`, and the copy strips CR so Windows-edited sources are
valid there. Override the staging directory with `AGENTS_BUILD_DIR`.
