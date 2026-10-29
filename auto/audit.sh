#!/bin/sh
# Audit the locked, resolved workspace graph, including all features/targets.
# Cargo.lock retains disabled optional dependencies; project only active entries
# without ignoring advisory IDs or changing the committed lockfile.
set -eu

database=
case "$#" in
    0) ;;
    2)
        if [ "$1" != --db ]; then
            printf '%s\n' 'Usage: ./auto/audit.sh [--db PATH]' >&2
            exit 1
        fi
        database=$(cd "$2" && pwd)
        ;;
    *) printf '%s\n' 'Usage: ./auto/audit.sh [--db PATH]' >&2; exit 1 ;;
esac
cd "$(dirname "$0")/.."

temporary=$(mktemp -d "${TMPDIR:-/tmp}/commonroom-audit.XXXXXX")
trap 'rm -rf "$temporary"' EXIT
trap 'exit 1' HUP INT TERM

cargo tree --locked --all-features --target all --prefix none --format '{p}' --color never > "$temporary/tree"

# Cargo generates package blocks separated by blank lines. Preserve their TOML
# verbatim except dependency edges to inactive packages. Keep every source when
# a name/version occurs more than once, as a conservative audit projection.
awk -v tree="$temporary/tree" '
    function fail(message) {
        print "Audit projection failed: " message > "/dev/stderr"
        failed = 1
        exit 1
    }
    BEGIN {
        while ((getline line < tree) > 0) {
            split(line, fields, " ")
            if (fields[2] ~ /^v/) {
                active[fields[1] SUBSEP substr(fields[2], 2)] = 1
                names[fields[1]] = 1
            }
        }
        close(tree)
        for (key in active) count++
        if (!count) fail("empty resolved dependency graph")
        RS = ""
        ORS = "\n\n"
    }
    {
        if ($0 !~ /^\[\[package\]\]/) {
            if (NR != 1 || $0 !~ /version = [34]/)
                fail("unsupported Cargo.lock format")
            print
            next
        }
        total++
        length_of_block = split($0, lines, "\n")
        name = version = ""
        for (i = 1; i <= length_of_block; i++) {
            split(lines[i], quoted, "\"")
            if (lines[i] ~ /^name = /) name = quoted[2]
            if (lines[i] ~ /^version = /) version = quoted[2]
        }
        key = name SUBSEP version
        if (!(key in active)) next
        seen[key] = 1
        selected++
        dependencies = 0
        for (i = 1; i <= length_of_block; i++) {
            if (lines[i] ~ /^replace = /)
                fail("resolve replacement packages before auditing")
            if (lines[i] == "dependencies = [") dependencies = 1
            else if (lines[i] == "]") dependencies = 0
            else if (dependencies) {
                split(lines[i], quoted, "\"")
                parts = split(quoted[2], dependency, " ")
                if (parts == 1) {
                    if (!(dependency[1] in names)) continue
                } else if (!((dependency[1] SUBSEP dependency[2]) in active)) continue
            }
            printf "%s\n", lines[i]
        }
        printf "\n"
    }
    END {
        if (failed) exit 1
        for (key in active)
            if (!(key in seen)) fail("resolved packages do not match Cargo.lock")
        printf "Auditing %d resolved packages; %d unused lockfile entries excluded.\n", selected, total - selected > "/dev/stderr"
    }
' Cargo.lock > "$temporary/Cargo.lock"

if [ -z "$database" ]; then
    database="$temporary/advisory-db"
    git clone --depth=1 https://github.com/RustSec/advisory-db.git "$database"
fi
cargo audit --db "$database" --no-fetch --file "$temporary/Cargo.lock"
