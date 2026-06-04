#!/usr/bin/env python3
"""Bump version across the recorder's package manifests.

Usage:
    python3 scripts/bump_version.py <patch|minor|major|X.Y.Z> [--all]

Without ``--all`` bumps only the production recorder (Rust + Python
pair, version-locked by ``scripts/check_recorder_version.py``). With
``--all`` also bumps the workspace root ``pyproject.toml`` and the
pure-Python reference recorder. Each manifest is bumped relative to
its own current version.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]

VERSION_RE = re.compile(r"^\d+\.\d+\.\d+$")
SEMVER_LINE = re.compile(
    r'(^\s*version\s*=\s*")(\d+\.\d+\.\d+)(")',
    re.MULTILINE,
)


def _bump(component: str, current: str) -> str:
    if VERSION_RE.match(component):
        return component
    a, b, p = (int(x) for x in current.split("."))
    if component == "major":
        return f"{a + 1}.0.0"
    if component == "minor":
        return f"{a}.{b + 1}.0"
    if component == "patch":
        return f"{a}.{b}.{p + 1}"
    raise SystemExit(f"unknown component {component!r}")


def _patch_first_version_line(path: Path, component: str) -> tuple[str, str] | None:
    text = path.read_text(encoding="utf-8")
    match = SEMVER_LINE.search(text)
    if match is None:
        return None
    current = match.group(2)
    new = _bump(component, current)
    if new == current:
        return current, new
    new_text = SEMVER_LINE.sub(
        lambda m: f"{m.group(1)}{new}{m.group(3)}" if m.group(2) == current else m.group(0),
        text,
        count=1,
    )
    path.write_text(new_text, encoding="utf-8")
    return current, new


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("component", help="patch | minor | major | X.Y.Z")
    parser.add_argument(
        "--all",
        action="store_true",
        help="also bump the workspace root and the pure-Python reference recorder",
    )
    args = parser.parse_args()

    targets = [
        REPO_ROOT / "codetracer-python-recorder" / "Cargo.toml",
        REPO_ROOT / "codetracer-python-recorder" / "pyproject.toml",
    ]
    if args.all:
        targets.extend(
            [
                REPO_ROOT / "pyproject.toml",
                REPO_ROOT / "codetracer-pure-python-recorder" / "pyproject.toml",
            ]
        )

    missing = [p for p in targets if not p.exists()]
    if missing:
        sys.stderr.write(
            "bump_version: missing manifest(s):\n  "
            + "\n  ".join(str(p) for p in missing)
            + "\n"
        )
        return 1

    changed = False
    for path in targets:
        result = _patch_first_version_line(path, args.component)
        rel = path.relative_to(REPO_ROOT)
        if result is None:
            sys.stderr.write(f"bump_version: no version line found in {rel}\n")
            return 1
        current, new = result
        if current == new:
            print(f"[bump-version] {rel}: {current} (unchanged)")
        else:
            print(f"[bump-version] {rel}: {current} -> {new}")
            changed = True

    return 0 if changed else 0


if __name__ == "__main__":
    raise SystemExit(main())
