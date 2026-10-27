#!/usr/bin/env python3
"""Audit the locked, resolved workspace graph, including all workspace features.

Cargo.lock also records unused optional backends (notably SQLx MySQL/RSA). Keep
those out of the audit projection without ignoring any advisory: enabling one
later puts it into the resolved graph and therefore into this audit automatically.
Requires Python 3.11+, Git, Cargo and cargo-audit. Never modifies Cargo.lock.
"""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
try:
    import tomllib
except ModuleNotFoundError:
    raise SystemExit("Audit requires Python 3.11+; use python3.14 auto/audit.py on macOS.")

ROOT = Path(__file__).resolve().parent.parent

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, help="use an already-fetched RustSec Git database")
    args = parser.parse_args()
    # Cargo metadata's lock graph can retain disabled optional dependencies.
    # cargo tree selects the actual feature graph; include every target platform.
    tree = subprocess.check_output([
        "cargo", "tree", "--locked", "--all-features", "--target", "all",
        "--prefix", "none", "--format", "{p}", "--color", "never",
    ], cwd=ROOT, text=True)
    active = set()
    for line in tree.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            active.add((parts[0], parts[1][1:]))
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    known = {(package["name"], package["version"]) for package in lock["package"]}
    if not active or not active <= known:
        raise RuntimeError("Resolved packages do not match Cargo.lock")
    # Conservatively retain every source if a name/version has multiple sources.
    packages = [package for package in lock["package"]
                if (package["name"], package["version"]) in active]
    if any("replace" in package for package in packages):
        raise RuntimeError("Resolve replacement packages before auditing")
    names = {package["name"] for package in packages}
    versions = {(package["name"], package["version"]) for package in packages}
    def selected(dependency):
        parts = dependency.split()
        return parts[0] in names if len(parts) == 1 else (parts[0], parts[1]) in versions
    lines = ["# Read-only audit projection of the resolved Cargo.lock graph", f"version = {lock['version']}"]
    for package in packages:
        lines.append("\n[[package]]")
        for key in ["name", "version", "source", "checksum"]:
            if key in package:
                lines.append(f"{key} = {json.dumps(package[key])}")
        if "dependencies" in package:
            dependencies = [dependency for dependency in package["dependencies"] if selected(dependency)]
            lines.append(f"dependencies = {json.dumps(dependencies)}")
    with tempfile.TemporaryDirectory(prefix="commonroom-audit-") as temporary:
        folder = Path(temporary)
        projection = folder / "Cargo.lock"
        projection.write_text("\n".join(lines) + "\n")
        database = args.db.resolve() if args.db else folder / "advisory-db"
        if args.db is None:
            subprocess.run(["git", "clone", "--depth=1", "https://github.com/RustSec/advisory-db.git", str(database)], check=True)
        print(f"Auditing {len(packages)} resolved packages; {len(lock['package']) - len(packages)} unused lockfile entries excluded.", flush=True)
        subprocess.run(["cargo", "audit", "--db", str(database), "--no-fetch", "--file", str(projection)], cwd=ROOT, check=True)

if __name__ == "__main__":
    main()
