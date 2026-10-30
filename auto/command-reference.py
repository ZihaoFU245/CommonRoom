#!/usr/bin/env python3
"""Generate/check the public command catalogue without opening server data."""
import argparse
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    result = subprocess.run(
        ["cargo", "run", "--quiet", "--locked", "--", "--command-reference"],
        cwd=ROOT, check=False, capture_output=True, text=True,
    )
    if result.returncode:
        raise SystemExit(result.stderr or "Command reference export failed")
    target = ROOT / "docs" / "command-reference.md"
    if args.check:
        if not target.is_file() or target.read_text() != result.stdout:
            raise SystemExit("Command reference is stale; run python3 auto/command-reference.py")
        print("Command reference matches the server catalogue")
    else:
        target.write_text(result.stdout)
        print("Updated docs/command-reference.md")

if __name__ == "__main__":
    main()
