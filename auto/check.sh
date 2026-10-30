#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

# Run all source checks before creating frontend or release artifacts.
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 auto/command-reference.py --check
node --check web/scripts/smoke.mjs
node --check web/scripts/media-smoke.mjs
sh -n auto/start-s3.sh
sh -n auto/start-postgres.sh
sh -n auto/debug.sh
pnpm --dir web check
pnpm --dir web test
