#!/usr/bin/env python3
"""Compare the current persistence benchmark with the v5 baseline in isolation."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--baseline-ref', default='181fd92c88bed009f7a67afad393c96b2d7a8bdf')
args = parser.parse_args()


def run_benchmark(tree, label):
    env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / 'target'))
    build = subprocess.run(
        ['cargo', 'test', '--manifest-path', str(tree / 'Cargo.toml'), '--release',
         '--locked', '--bin', 'chat-storage-baseline' if label == 'baseline' else 'chat',
         'mutation_persistence_benchmark', '--no-run', '--message-format=json'],
        check=True, text=True, stdout=subprocess.PIPE, env=env,
    )
    executables = [item['executable'] for line in build.stdout.splitlines()
                   if (item := json.loads(line)).get('reason') == 'compiler-artifact'
                   and item.get('executable')]
    process = subprocess.Popen([executables[-1], 'mutation_persistence_benchmark',
                                '--ignored', '--nocapture'], stdout=subprocess.PIPE, text=True)
    output = process.stdout.read()
    _, status, usage = os.wait4(process.pid, 0)
    process.returncode = os.waitstatus_to_exitcode(status)
    print(f'{label}:\n{output}', flush=True)
    if process.returncode:
        raise subprocess.CalledProcessError(process.returncode, process.args)
    rss_bytes = usage.ru_maxrss if sys.platform == 'darwin' else usage.ru_maxrss * 1024
    print(f'{label}_peak_rss_bytes={rss_bytes}', flush=True)


with tempfile.TemporaryDirectory(prefix='chatroom-storage-benchmark-') as directory:
    baseline = Path(directory)
    archive = subprocess.Popen(['git', '-C', str(ROOT), 'archive', args.baseline_ref], stdout=subprocess.PIPE)
    subprocess.run(['tar', '-x', '-C', str(baseline)], stdin=archive.stdout, check=True)
    archive.stdout.close()
    if archive.wait():
        raise RuntimeError('git archive failed')
    manifest = baseline / 'server/Cargo.toml'
    manifest.write_text(manifest.read_text() + '\n[[bin]]\nname = \"chat-storage-baseline\"\npath = \"src/main.rs\"\n')
    engine = baseline / 'server/src/engine'
    tests = engine / 'tests.rs'
    tests.write_text(tests.read_text().replace('fn engine() -> Engine', 'pub(super) fn engine() -> Engine'))
    modules = engine / 'mod.rs'
    modules.write_text(modules.read_text() + '\n#[cfg(test)]\n#[allow(clippy::unwrap_used, clippy::expect_used)]\nmod benchmarks;\n')
    benchmark = (ROOT / 'server/src/engine/benchmarks.rs').read_text()
    # Adapt only fixture access to the old lock/layout; timed commands are identical.
    benchmark = benchmark.replace('.db.lock().unwrap()', '.db').replace('.db.lock()\n            .unwrap()', '.db')
    benchmark = benchmark.replace('super::tests::room_messages(&e, "room0")', 'e.data.rooms["room0"].messages')
    benchmark = benchmark.replace('super::tests::private_messages(&e, "alice:peer0")', 'e.data.private["alice:peer0"].messages')
    (engine / 'benchmarks.rs').write_text(benchmark)
    run_benchmark(baseline, 'baseline')
    run_benchmark(ROOT, 'current')
