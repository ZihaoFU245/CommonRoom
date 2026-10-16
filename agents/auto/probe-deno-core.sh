#!/bin/sh
# Gate: can `deno_core` build and start a V8 isolate in this environment?
#
# Usage: agents/auto/probe-deno-core.sh
#
# Self-contained on purpose: it generates its own probe crate under $HOME/deno-probe
# rather than consuming files staged from the checkout, so it works when the script
# is piped to `sh`, copied to /tmp, or run from any working directory. POSIX sh only:
# no heredocs (dash does not support them the way bash does), no bashisms.
#
# It documents the two things that cost real time here: linking rusty_v8 needs a
# system C++ toolchain plus libc++, and the build must happen on a Linux filesystem
# (not /mnt/d, where the link step and file watching are unreliable).
set -eu

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

probe=${DENO_PROBE_DIR:-$HOME/deno-probe}

printf '%s\n' "== toolchain =="
printf 'clang:   %s\n' "$(clang --version 2>/dev/null | head -1 || echo MISSING)"
printf 'cargo:   %s\n' "$(cargo --version 2>/dev/null || echo MISSING)"
printf 'rustc:   %s\n' "$(rustc --version 2>/dev/null || echo MISSING)"
printf 'python3: %s\n' "$(python3 --version 2>/dev/null || echo MISSING)"

if ! command -v clang >/dev/null 2>&1; then
    printf '%s\n' "clang is required to link rusty_v8." >&2
    printf '%s\n' "Install: sudo apt-get install -y clang libc++-dev libc++abi-dev cmake ninja-build pkg-config" >&2
    exit 1
fi

rm -rf "$probe"
mkdir -p "$probe/src"

{
    printf '%s\n' '[package]'
    printf '%s\n' 'name = "deno-probe"'
    printf '%s\n' 'version = "0.0.0"'
    printf '%s\n' 'edition = "2024"'
    printf '\n'
    printf '%s\n' '[dependencies]'
    printf '%s\n' 'deno_core = "0"'
} > "$probe/Cargo.toml"

{
    printf '%s\n' '//! Minimal gate: does a `deno_core` JsRuntime start and evaluate a script here?'
    printf '\n'
    printf '%s\n' 'fn main() {'
    printf '%s\n' '    let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions::default());'
    printf '%s\n' '    runtime'
    printf '%s\n' '        .execute_script('
    printf '%s\n' '            "<probe>",'
    printf '%s\n' '            deno_core::FastString::from_static('
    printf '%s\n' '                "Deno.core.print(`deno_core probe: 1 + 1 = ${1 + 1}\n`);",'
    printf '%s\n' '            ),'
    printf '%s\n' '        )'
    printf '%s\n' '        .expect("evaluate probe script");'
    printf '%s\n' '    println!("deno_core probe: JsRuntime started and evaluated a script");'
    printf '%s\n' '}'
} > "$probe/src/main.rs"

printf '%s\n' "== building $probe =="
cd "$probe"
cargo build 2>&1 | tail -15 || {
    printf '%s\n' "PROBE FAILED: deno_core did not build." >&2
    exit 1
}

printf '%s\n' "== resolved versions =="
for name in deno_core v8; do
    version=$(grep -A1 "^name = \"$name\"$" Cargo.lock | sed -n 's/^version = "\(.*\)"/\1/p')
    printf '%-12s %s\n' "$name" "${version:-unresolved}"
done

printf '%s\n' "== run =="
./target/debug/deno-probe
