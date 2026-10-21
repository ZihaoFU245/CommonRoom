#!/bin/sh
# Starts only a manually installed local S3 server. Never downloads anything.
set -eu
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
# Clean environment prevents inherited RustFS output paths escaping tmp/.
# Filesystem confinement also covers undocumented/default output paths.
set -- env -i PATH="$PATH" \
    RUSTFS_ACCESS_KEY="${RUSTFS_ACCESS_KEY:-cr}" \
    RUSTFS_SECRET_KEY="${RUSTFS_SECRET_KEY:-pass}" \
    RUSTFS_CONSOLE_ENABLE=true \
    RUSTFS_LOG_DIRECTORY="$runtime/logs" \
    RUSTFS_OBS_LOG_DIRECTORY="$runtime/logs" \
    TMPDIR="$runtime/temp" TMP="$runtime/temp" TEMP="$runtime/temp" \
    "$repo/tmp/rustfs" server --address 127.0.0.1:19000 \
    --console-address 127.0.0.1:19001 "$runtime/data"
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
