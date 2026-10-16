# Agents

One Rust process — the **runner** — embeds V8 and runs one isolate per agent, in
the Cloudflare-Worker shape. The runner is the only component holding a model key
or a chat credential; agents reach both through host calls they cannot forge.

```text
agents/
  runner/     the runner: config, workspaces, isolates, chat client, console
  guest/      the rig agent compiled to wasm32-unknown-unknown, loaded by an isolate
  auto/       POSIX sh: build, check, toolchain gate
```

This replaced an earlier design that ran one `deno run` OS process per agent under
a TypeScript supervisor. That code is gone; nothing here depends on it.

## Start here

- [runner/README.md](runner/README.md) — what works today, layout, build, and the
  exact list of pieces still missing.
- [../docs/agents.md](../docs/agents.md) — the operator guide. Note it still
  describes the removed Deno supervisor; treat it as background until it is
  rewritten for the runner.

## Data layout

Everything the runner owns lives under the repository's top-level `data/`
directory, which is already gitignored:

```text
data/config.json                  runner settings under the `agents` key
data/workspaces/workspace<Name>/  one agent: descriptor, credential, state, files
```

`data/config.json` is shared with the chat server. The runner reads and writes
only its own `agents` key, so `bind`, `origins` and the rest survive untouched.
Point it elsewhere with `--config` or `AGENTS_CONFIG`.

## Build and check

`rusty_v8` needs a C++ toolchain and libc++:

```sh
sudo apt-get install -y clang libc++-dev libc++abi-dev cmake ninja-build pkg-config
agents/auto/probe-deno-core.sh    # gate: does deno_core build and run here?
agents/auto/check.sh              # fmt, clippy -D warnings, tests
agents/auto/build.sh debug        # runner binary
agents/auto/demo-cli.sh           # walk the CLI end to end
```

The scripts are POSIX `sh` and copy the tree to a Linux filesystem before
building, because cargo cannot link reliably on `/mnt/d` and Windows line endings
are not valid there.
