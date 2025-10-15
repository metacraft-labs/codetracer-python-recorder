#!/usr/bin/env python3
"""Fail the build when unchecked `.unwrap(` usage appears outside the allowlist."""
from __future__ import annotations

import argparse
import pathlib
import sys
from typing import Iterable

DEFAULT_ALLOWLIST = {
    pathlib.Path("codetracer-python-recorder/src/runtime/value_encoder.rs"),
    pathlib.Path("codetracer-python-recorder/src/runtime/mod.rs"),
    pathlib.Path("codetracer-python-recorder/src/monitoring/mod.rs"),
    pathlib.Path("codetracer-python-recorder/src/monitoring/tracer.rs"),
}


def scan_for_unsafe_unwraps(root: pathlib.Path, allowlist: Iterable[pathlib.Path]) -> list[pathlib.Path]:
    repo_root = root.resolve()
    src_root = repo_root / "codetracer-python-recorder" / "src"
    allowed = {path.resolve() for path in (repo_root / entry for entry in allowlist)}

    failures: list[pathlib.Path] = []
    for candidate in src_root.rglob("*.rs"):
        if candidate in allowed:
            continue
        text = candidate.read_text()
        if ".unwrap(" in text:
            failures.append(candidate.relative_to(repo_root))
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo-root",
        type=pathlib.Path,
        default=pathlib.Path(__file__).resolve().parents[2],
        help="Path to the repository root (auto-detected by default)",
    )
    parser.add_argument(
        "--allow",
        action="append",
        dest="allow",
        default=[str(path) for path in DEFAULT_ALLOWLIST],
        help="Additional relative paths that may contain unwrap usage",
    )
    args = parser.parse_args()

    allowlist = [pathlib.Path(entry) for entry in args.allow]
    failures = scan_for_unsafe_unwraps(args.repo_root, allowlist)
    if failures:
        print("Found disallowed `.unwrap(` usage in the recorder crate:", file=sys.stderr)
        for failure in sorted(failures):
            print(f" - {failure}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
