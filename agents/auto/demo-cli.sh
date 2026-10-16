#!/bin/sh
# End-to-end CLI demonstration for the agent runner.
#
# Proves the operator-visible contract without a chat server:
#   1. a first run creates data/config.json with default values
#   2. an existing config written by the chat server keeps every server key
#   3. `list` is read-only and reports an empty installation honestly
#   4. the files the runner writes are owner-only
#
# Usage: agents/auto/demo-cli.sh [path-to-runner-binary]
set -eu

runner=${1:-}
if [ -z "$runner" ]; then
    stage=${AGENTS_BUILD_DIR:-$HOME/.cache/agents-build}
    runner=$stage/src/target/debug/runner
fi
if [ ! -x "$runner" ]; then
    printf '%s\n' "No runner binary at $runner. Run agents/auto/build.sh debug first." >&2
    exit 1
fi

work=${DEMO_DIR:-$HOME/runner-demo}
rm -rf "$work"
mkdir -p "$work"

printf '%s\n' "=== 1. first run creates the configuration with defaults ==="
"$runner" config --config "$work/data/config.json"
printf '\n'

printf '%s\n' "=== 2. a chat-server config keeps its own keys ==="
printf '%s\n' '{"bind":"127.0.0.1:3000","origins":["http://localhost:5173"],"production":false}' \
    > "$work/data/config.json"
"$runner" config --config "$work/data/config.json" > /dev/null
printf '%s\n' '--- data/config.json after the runner touched it ---'
head -8 "$work/data/config.json"
printf '\n'

printf '%s\n' "=== 3. list reports an empty installation ==="
"$runner" list --config "$work/data/config.json"
printf '\n'

printf '%s\n' "=== 4. permissions of what the runner wrote ==="
stat -c '%a %n' "$work/data/config.json"

printf '\n%s\n' "=== 5. usage ==="
"$runner" --help | head -12
