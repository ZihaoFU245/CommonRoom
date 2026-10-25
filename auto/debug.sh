#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

help() {
    cat <<'HELP'
Usage: ./auto/debug.sh [--with-s3] [--with-postgres]

Local:
  ./auto/start-s3.sh
  ./auto/start-postgres.sh

Apple container:
  mkdir -p pgdata
  container run -d \
    --name postgres \
    --memory 1G \
    --cpus 2 \
    -e POSTGRES_USER=cr \
    -e POSTGRES_PASSWORD=pass \
    -e POSTGRES_DB=devdb \
    -v "$PWD/pgdata:/var/lib/postgresql" \
    -p 127.0.0.1:5432:5432 \
    docker.io/library/postgres:18

Docker:
  mkdir -p pgdata
  docker run -d \
    --name postgres \
    --memory 1g \
    --cpus 2 \
    -e POSTGRES_USER=cr \
    -e POSTGRES_PASSWORD=pass \
    -e POSTGRES_DB=devdb \
    -v "$PWD/pgdata:/var/lib/postgresql" \
    -p 127.0.0.1:5432:5432 \
    docker.io/library/postgres:18

macOS VPN route (Apple Container):
  sudo route -n change -net 192.168.64.0/24 -interface bridge100
  # If missing:
  sudo route -n add -net 192.168.64.0/24 -interface bridge100

HELP
}

with_s3=false
backend=sqlite
for arg in "$@"; do
    case "$arg" in
        --with-s3) with_s3=true ;;
        --with-postgres) backend=postgres ;;
        -h|--help) help; exit 0 ;;
        *) echo "Unknown option: $arg" >&2; exit 2 ;;
    esac
done
if [ ! -x target/debug/chat ] || [ ! -f web/dist-debug/index.html ]; then
    echo 'Run ./auto/build.sh debug first.' >&2
    exit 1
fi

# Use the repository's persistent debug data/config.
umask 077
export CHAT_DATA="$PWD/data"
export CHAT_PRODUCTION=0 CHAT_BIND=127.0.0.1:3000 CHAT_BASE_URL=/
export CHAT_TRUST=127.0.0.1 CHAT_DIRECT_BOT_HTTP=0
mkdir -p "$CHAT_DATA"
printf '{"production":false,"database":{"backend":"%s"},"s3":null}\n' "$backend" > "$CHAT_DATA/config.json"
if [ "$backend" = postgres ]; then
    export CHAT_DATABASE_URL="${CHAT_DATABASE_URL:-postgresql://cr:pass@127.0.0.1:5432/devdb}"
else
    unset CHAT_DATABASE_URL
fi

server_pid=
ui_pid=
cleanup() {
    trap '' INT TERM
    for pid in "$server_pid" "$ui_pid"; do
        if [ -n "$pid" ]; then
            kill "$pid" 2>/dev/null || :
            wait "$pid" 2>/dev/null || :
        fi
    done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [ "$with_s3" = true ]; then
    export CHAT_S3_ENDPOINT=http://127.0.0.1:19000 CHAT_S3_PATH_STYLE=true
    export CHAT_S3_PUBLIC_ENDPOINT="$CHAT_S3_ENDPOINT"
    export CHAT_S3_BUCKET="${CHAT_S3_BUCKET:-commonroom-draft}"
    export CHAT_S3_REGION=us-east-1
    export CHAT_S3_ACCESS_KEY="${RUSTFS_ACCESS_KEY:-cr}"
    export CHAT_S3_SECRET_KEY="${RUSTFS_SECRET_KEY:-pass}"
    unset CHAT_S3_SESSION_TOKEN
    ./target/debug/chat --setup-local-s3 </dev/null
else
    unset CHAT_S3_ENDPOINT CHAT_S3_PUBLIC_ENDPOINT CHAT_S3_BUCKET CHAT_S3_REGION \
        CHAT_S3_PATH_STYLE CHAT_S3_ACCESS_KEY CHAT_S3_SECRET_KEY CHAT_S3_SESSION_TOKEN
fi

# Node is only the existing UI preview; chat remains the Rust debug binary.
exec 3<&0
node web/node_modules/vite/bin/vite.js preview web --outDir dist-debug \
    --host 127.0.0.1 --port 5173 --strictPort </dev/null 3<&- &
ui_pid=$!
./target/debug/chat <&3 3<&- &
server_pid=$!
exec 3<&-
printf 'Debug UI: http://127.0.0.1:5173 | database: %s | S3: %s\n' "$backend" "$with_s3"

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
