#!/bin/sh
# Full source check for the agent runner: format, lint, test.
#
# POSIX sh, no bashisms, no heredocs (dash is /bin/sh on Ubuntu). Runs from a
# Linux filesystem copy of the checkout: cargo cannot reliably watch or link on
# /mnt/d, and Windows line endings are stripped in the copy.
#
# Usage: agents/auto/check.sh
set -eu

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

work=${AGENTS_BUILD_DIR:-$HOME/.cache/agents-build}
stage=$work/src
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

rm -rf "$stage"
mkdir -p "$stage"
(cd "$repo/agents" && find . -type f ! -path './target/*' ! -name '*.wasm' -print) |
while IFS= read -r file; do
    mkdir -p "$stage/$(dirname "$file")"
    sed 's/\r$//' < "$repo/agents/$file" > "$stage/$file"
done

cd "$stage"

printf '%s\n' "== cargo fmt =="
cargo fmt --all

printf '%s\n' "== cargo clippy =="
cargo clippy --all-targets -- -D warnings

printf '%s\n' "== cargo test =="
cargo test

printf '%s\n' "== summary =="
printf '%s\n' "staged source: $stage"
printf '%s\n' "If cargo fmt changed files, copy $stage/runner/src back into agents/runner/src."
