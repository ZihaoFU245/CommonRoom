#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

mode=${1:-debug}
if [ "$#" -gt 1 ]; then
    printf '%s\n' 'Usage: ./auto/check.sh [debug|release]' >&2
    exit 1
fi
case "$mode" in
    debug) set -- ;;
    release)
        set -- --release
        # Optimized tests enforce production startup; use the fixture origin.
        export CHAT_ORIGIN=https://chat.example.com
        ;;
    *) printf '%s\n' 'Usage: ./auto/check.sh [debug|release]' >&2; exit 1 ;;
esac

# Run all source checks before creating frontend or release artifacts.
cargo fmt --all --check
cargo clippy "$@" --all-targets --locked -- -D warnings
cargo test "$@" --locked
node --check web/scripts/smoke.mjs
node --check web/scripts/media-smoke.mjs
sh -n auto/start-s3.sh
sh -n auto/start-postgres.sh
sh -n auto/debug.sh
pnpm --dir web check
pnpm --dir web test
