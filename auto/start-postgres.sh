#!/bin/sh
# Starts an installed PostgreSQL with all writable files confined to pgdata/.
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
case "${1:-}" in
    -h|--help)
        echo 'Usage: ./auto/start-postgres.sh'
        echo 'Starts PostgreSQL at 127.0.0.1:5432 (cr / pass, database devdb).'
        echo 'Keeps the cluster, logs, sockets and temporary files in ./pgdata/.'
        echo 'Ctrl+C stops PostgreSQL; subsequent runs reuse the cluster.'
        exit 0 ;;
    '') ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
esac
if ! command -v pg_config >/dev/null 2>&1; then
    echo 'Add the installed PostgreSQL bin directory to PATH first.' >&2
    exit 1
fi
pg_bin=$(pg_config --bindir)
for command in initdb postgres pg_ctl psql createdb; do
    if [ ! -x "$pg_bin/$command" ]; then
        echo "Missing PostgreSQL executable: $pg_bin/$command" >&2
        exit 1
    fi
done
# pg_ctl parses its server options as shell words; reject path metacharacters.
case "$repo" in *'"'*|*"'"*|*'\'*) echo 'Unsupported PostgreSQL runtime path.' >&2; exit 1;; esac
runtime="$repo/pgdata"
umask 077
if [ -L "$runtime" ]; then
    echo 'pgdata/ must not be a symlink.' >&2
    exit 1
fi
mkdir -p "$runtime"
cd "$runtime"

# Apply confinement to every PostgreSQL command, including cluster initialization.
run_pg() {
    set -- env -i PATH="$pg_bin:/usr/bin:/bin" HOME="$runtime" \
        TMPDIR="$runtime" TMP="$runtime" TEMP="$runtime" \
        LC_ALL=C PGPASSWORD=pass PGHOST=127.0.0.1 PGPORT=5432 \
        PGUSER=cr PGDATABASE=postgres PSQL_HISTORY=/dev/null "$@"
    case $(uname -s) in
        Darwin)
            sandbox-exec -p "(version 1)(allow default)(deny file-write*)(allow file-write* (subpath \"$runtime\"))(allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\"))" "$@"
            ;;
        Linux)
            if ! command -v bwrap >/dev/null 2>&1; then
                echo 'Install bubblewrap to confine PostgreSQL writes to pgdata/.' >&2
                exit 1
            fi
            bwrap --die-with-parent --ro-bind / / --bind "$runtime" "$runtime" --dev /dev --proc /proc "$@"
            ;;
        *) echo 'Filesystem confinement is supported on macOS and Linux only.' >&2; exit 1;;
    esac
}

if [ ! -f PG_VERSION ]; then
    printf '%s\n' pass | run_pg "$pg_bin/initdb" -D "$runtime" \
        --encoding=UTF8 --username=cr --auth=scram-sha-256 --pwfile=/dev/stdin --no-instructions
fi
# pg_ctl refuses a second start of this cluster; it never adopts another server.
started=false
cleanup() {
    trap '' INT TERM
    if [ "$started" = true ]; then
        run_pg "$pg_bin/pg_ctl" -D "$runtime" -m fast -w stop
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
run_pg "$pg_bin/pg_ctl" -D "$runtime" -l "$runtime/postgres.log" \
    -o "-h 127.0.0.1 -p 5432 -k '$runtime' -c data_directory='$runtime' -c log_directory='$runtime' -c external_pid_file=''" \
    -w start
started=true
if [ "$(run_pg "$pg_bin/psql" -X -Atqc "SELECT 1 FROM pg_database WHERE datname = 'devdb'")" != 1 ]; then
    run_pg "$pg_bin/createdb" devdb
fi
printf '%s\n' 'PostgreSQL: 127.0.0.1:5432 | user: cr | database: devdb' \
    "Data/logs/sockets/temp: $runtime | Ctrl+C to stop."
while run_pg "$pg_bin/pg_ctl" -D "$runtime" status >/dev/null 2>&1; do
    sleep 1
done
started=false
echo "PostgreSQL exited; inspect $runtime/postgres.log." >&2
exit 1
