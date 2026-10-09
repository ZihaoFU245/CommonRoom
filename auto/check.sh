#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

# Run all source checks before creating frontend or release artifacts.
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
pnpm --dir web check
pnpm --dir web test
