#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
if [ ! -x target/debug/chat ]; then
    printf '%s\n' 'Debug artifact missing. Run ./auto/build.sh debug first.' >&2
    exit 1
fi
if [ ! -f web/dist-debug/index.html ]; then
    printf '%s\n' 'Debug UI missing. Run ./auto/build.sh debug first.' >&2
    exit 1
fi
if ! command -v node >/dev/null 2>&1 || [ ! -f web/node_modules/vite/bin/vite.js ]; then
    printf '%s\n' 'Node and web dependencies are required. Run ./auto/build.sh debug first.' >&2
    exit 1
fi

server_pid=
ui_pid=
cleanup() {
    trap '' INT TERM
    if [ -n "$server_pid" ]; then
        kill "$server_pid" 2>/dev/null || :
        wait "$server_pid" 2>/dev/null || :
    fi
    if [ -n "$ui_pid" ]; then
        kill "$ui_pid" 2>/dev/null || :
        wait "$ui_pid" 2>/dev/null || :
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Save stdin explicitly: background commands otherwise receive /dev/null.
exec 3<&0
node web/node_modules/vite/bin/vite.js preview web --outDir dist-debug \
    --host 127.0.0.1 --port 5173 --strictPort </dev/null 3<&- &
ui_pid=$!
./target/debug/chat <&3 3<&- &
server_pid=$!
exec 3<&-
printf '%s\n' 'Debug UI: http://127.0.0.1:5173 (Ctrl+C stops UI and server).'

# POSIX sh has no wait -n; watch both children and propagate the first exit.
while kill -0 "$server_pid" 2>/dev/null && kill -0 "$ui_pid" 2>/dev/null; do
    sleep 1
done
status=0
if ! kill -0 "$server_pid" 2>/dev/null; then
    wait "$server_pid" || status=$?
else
    wait "$ui_pid" || status=$?
fi
exit "$status"
