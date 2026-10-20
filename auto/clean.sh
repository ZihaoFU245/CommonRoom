#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

if [ -L tmp ]; then
    echo "tmp/ must not be a symlink." >&2
    exit 1
fi

rm -rf -- target web/dist web/dist-debug \
    web/node_modules/.vite web/node_modules/.vite-temp chat ui.tar.xz \
    tmp/rustfs-local
printf '%s\n' 'Removed server/UI build artifacts, Vite caches and local RustFS data/log/temp files.' \
    'Preserved tmp/rustfs and other files in tmp/.'
