#!/bin/sh
# Build the agent runner and its wasm guest.
#
# POSIX sh on purpose: this runs in WSL, in a container, or on any Linux host.
# The project is edited on Windows, so every tree that reaches cargo is first
# copied to a Linux filesystem with CRLF normalized away.
#
# Usage: agents/auto/build.sh [debug|release] [--guest] [--check]
set -eu

mode=release
guest=0
check_only=0
for arg in "$@"; do
    case "$arg" in
        debug|release) mode=$arg ;;
        --guest) guest=1 ;;
        --check) check_only=1 ;;
        *) printf 'Usage: %s [debug|release] [--guest] [--check]\n' "$0" >&2; exit 2 ;;
    esac
done

# The checkout root. These scripts are often staged to a temporary directory
# before they run, so $0 cannot be trusted; ask git first, then walk up.
repo=${AGENTS_REPO:-}
if [ -z "$repo" ]; then
    script_dir=$(cd "$(dirname "$0")" && pwd)
    if [ -d "$script_dir/../runner" ]; then
        repo=$(cd "$script_dir/.." && pwd)
    else
        repo=$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null || true)
    fi
fi
if [ -z "$repo" ] || [ ! -d "$repo/agents/runner" ]; then
    printf '%s\n' "Cannot locate the agents/ tree. Set AGENTS_REPO=<repo root>." >&2
    exit 1
fi
agents=$repo/agents
work=${AGENTS_BUILD_DIR:-$HOME/.cache/agents-build}
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

# Cargo needs a Linux filesystem: /mnt/d is drvfs, where the file watcher and
# the V8 link step are both unreliable and slow.
stage=$work/src
rm -rf "$stage"
mkdir -p "$stage"

copy_tree() {
    src=$1
    dst=$2
    mkdir -p "$dst"
    (cd "$src" && find . -type f \
        ! -path './target/*' \
        ! -name '*.wasm' \
        ! -path './auto/probe/*' \
        -print) | while IFS= read -r file; do
        mkdir -p "$dst/$(dirname "$file")"
        # Strip CR so shell scripts and Rust sources are valid on Linux. `sed`
        # rather than `tr`: the escape `\r` is literal under dash's `tr`, which
        # would delete the characters r, 0, 1 and 5 instead of carriage returns.
        sed 's/\r$//' < "$src/$file" > "$dst/$file"
    done
}

copy_tree "$agents" "$stage"
printf '%s\n' "staged $agents -> $stage"

cd "$stage"

if [ "$check_only" = 1 ]; then
    cargo fmt --all --check
    cargo clippy --all-targets --locked -- -D warnings
    cargo test --locked
    exit 0
fi

if [ "$guest" = 1 ]; then
    rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
    if ! command -v wasm-bindgen >/dev/null 2>&1; then
        printf '%s\n' 'wasm-bindgen not found. Run agents/auto/setup.sh first.' >&2
        exit 1
    fi
    cargo build -p guest --target wasm32-unknown-unknown --release
    wasm-bindgen --target web --out-dir "$agents/guest/wasm" \
        "$stage/target/wasm32-unknown-unknown/release/guest.wasm"
    printf '%s\n' "guest written to $agents/guest/wasm"
fi

if [ "$mode" = debug ]; then
    cargo build --locked
    printf '%s\n' "Built $stage/target/debug/runner"
else
    cargo build --release --locked
    printf '%s\n' "Built $stage/target/release/runner"
fi