#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

rm -rf -- target web/dist web/dist-debug \
    web/node_modules/.vite web/node_modules/.vite-temp chat ui.tar.xz
printf '%s\n' 'Removed server/UI build artifacts and Vite caches.'
