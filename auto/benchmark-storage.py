#!/usr/bin/env python3
"""Compare HEAD-era persistence benchmarks against the working tree in isolation."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
BENCHMARK = Path('server/src/engine/tests/benchmarks.rs')


def run_benchmark(tree, label):
    target = 'chat-storage-baseline' if label == 'baseline' else 'chat'
    env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / 'target'))
    print(f'Building {label} release benchmark...', flush=True)
    build = subprocess.run(
        ['cargo', 'test', '--manifest-path', str(tree / 'Cargo.toml'), '--release',
         '--locked', '--bin', target, 'mutation_persistence_benchmark',
         '--no-run', '--message-format=json'],
        check=True, text=True, stdout=subprocess.PIPE, env=env,
    )
    executables = [item['executable'] for line in build.stdout.splitlines()
                   if (item := json.loads(line)).get('reason') == 'compiler-artifact'
                   and item.get('executable') and item['target']['name'] == target]
    if len(executables) != 1:
        raise RuntimeError(f'Expected one {label} benchmark executable, found {executables}')
    process = subprocess.Popen(
        [executables[0], 'mutation_persistence_benchmark', '--ignored', '--nocapture'],
        stdout=subprocess.PIPE, text=True,
    )
    output = process.stdout.read()
    process.stdout.close()
    _, status, usage = os.wait4(process.pid, 0)
    process.returncode = os.waitstatus_to_exitcode(status)
    print(f'{label}:\n{output}', flush=True)
    if process.returncode:
        raise subprocess.CalledProcessError(process.returncode, process.args)
    rss_bytes = usage.ru_maxrss if sys.platform == 'darwin' else usage.ru_maxrss * 1024
    print(f'{label}_peak_rss_bytes={rss_bytes}', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-ref', default='HEAD',
                        help='Compatible HEAD-era revision to compare (default: HEAD)')
    args = parser.parse_args()
    benchmark = (ROOT / BENCHMARK).read_text()
    revision = subprocess.check_output(
        ['git', '-C', str(ROOT), 'rev-parse', '--verify', f'{args.baseline_ref}^{{commit}}'],
        text=True,
    ).strip()
    print(f'baseline_revision={revision}', flush=True)
    with tempfile.TemporaryDirectory(prefix='chatroom-storage-benchmark-') as directory:
        baseline = Path(directory)
        # Export only the server and its compile-time fixtures. The optional
        # simpleBot worker, working data and checkout state are not involved.
        archive = subprocess.Popen(
            ['git', '-C', str(ROOT), 'archive', revision, 'Cargo.toml', 'Cargo.lock',
             'server', 'docs', 'web/tests/fixtures'], stdout=subprocess.PIPE,
        )
        try:
            subprocess.run(['tar', '-x', '-C', str(baseline)],
                           stdin=archive.stdout, check=True)
        finally:
            archive.stdout.close()
            status = archive.wait()
        if status:
            raise RuntimeError('git archive failed')
        destination = baseline / BENCHMARK
        if not destination.is_file():
            raise RuntimeError('Baseline predates the supported HEAD-era benchmark layout')
        manifest = baseline / 'server/Cargo.toml'
        manifest.write_text(manifest.read_text()
                            + '\n[[bin]]\nname = "chat-storage-baseline"\npath = "src/main.rs"\n')
        # Both versions execute exactly the same fixture and timed commands.
        # Distinct binary names prevent Cargo from reusing the other's executable.
        destination.write_text(benchmark)
        run_benchmark(baseline, 'baseline')
        run_benchmark(ROOT, 'current')


if __name__ == '__main__':
    main()
