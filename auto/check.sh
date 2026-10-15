#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

# Run all source checks before creating frontend or release artifacts.
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
node --check web/scripts/smoke.mjs
# The separately maintained demo is optional in a server-only checkout.
if [ -d simpleBot ]; then
    node web/node_modules/@typescript/native/bin/tsc -p simpleBot/tsconfig.json
    node --test simpleBot/test/*.test.ts
fi
pnpm --dir web check
pnpm --dir web test
