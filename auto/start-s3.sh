#!/bin/sh
# Starts only a manually installed local S3 server. Never downloads anything.
set -eu
bind_host=127.0.0.1
network_host=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --network)
            if [ "$#" -lt 2 ]; then
                echo '--network requires a LAN/Tailscale IPv4 address or hostname.' >&2
                exit 2
            fi
            shift
            case "$1" in
                ''|[!a-zA-Z0-9]*|*[!a-zA-Z0-9.-]*|0.0.0.0)
                    echo 'Invalid network host: use an IPv4 address or hostname without a scheme or port.' >&2
                    exit 2 ;;
            esac
            network_host=$1
            bind_host=0.0.0.0
            ;;
        -h|--help)
            echo 'Usage: ./auto/start-s3.sh [--network HOST]'
            echo 'Defaults to 127.0.0.1:19000 (API) and 127.0.0.1:19001 (console).'
            echo '--network HOST listens on all IPv4 interfaces for LAN/Tailscale debugging.'
            echo 'HOST is the LAN/Tailscale IPv4 address or hostname, without a scheme or port.'
            exit 0 ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
if [ -L "$repo/tmp" ]; then
    echo "tmp/ must not be a symlink." >&2
    exit 1
fi
if [ ! -x "$repo/tmp/rustfs" ] || [ -L "$repo/tmp/rustfs" ]; then
    echo "Download and extract RustFS for your OS/architecture to $repo/tmp/rustfs, then chmod +x it." >&2
    exit 1
fi
runtime="$repo/tmp/rustfs-local"
for path in "$runtime" "$runtime/data" "$runtime/logs" "$runtime/temp"; do
    if [ -L "$path" ]; then echo "RustFS runtime paths must not be symlinks." >&2; exit 1; fi
    mkdir -p "$path"
done
cd "$runtime"
if [ -n "$network_host" ]; then
    printf 'S3 API: http://%s:19000 | console: http://%s:19001\n' "$network_host" "$network_host"
fi
# Clean environment prevents inherited RustFS output paths escaping tmp/.
# Filesystem confinement also covers undocumented/default output paths.
set -- env -i PATH="$PATH" \
    RUSTFS_ACCESS_KEY="${RUSTFS_ACCESS_KEY:-cr}" \
    RUSTFS_SECRET_KEY="${RUSTFS_SECRET_KEY:-pass}" \
    RUSTFS_CONSOLE_ENABLE=true \
    RUSTFS_LOG_DIRECTORY="$runtime/logs" \
    RUSTFS_OBS_LOG_DIRECTORY="$runtime/logs" \
    TMPDIR="$runtime/temp" TMP="$runtime/temp" TEMP="$runtime/temp" \
    "$repo/tmp/rustfs" server --address "$bind_host:19000" \
    --console-address "$bind_host:19001" "$runtime/data"
case $(uname -s) in
    Darwin)
        # Quote the path as a sandbox string, rejecting characters needing escaping.
        case "$repo" in *'"'*|*'\'*) echo "Unsupported sandbox path." >&2; exit 1;; esac
        exec sandbox-exec -p "(version 1)(allow default)(deny file-write*)(allow file-write* (subpath \"$repo/tmp\"))(allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\"))" "$@"
        ;;
    Linux)
        if ! command -v bwrap >/dev/null 2>&1; then
            echo "Install bubblewrap to confine RustFS writes to tmp/." >&2
            exit 1
        fi
        exec bwrap --die-with-parent --ro-bind / / --bind "$repo/tmp" "$repo/tmp" --dev /dev --proc /proc "$@"
        ;;
    *) echo "Filesystem confinement is supported on macOS and Linux only." >&2; exit 1;;
esac
