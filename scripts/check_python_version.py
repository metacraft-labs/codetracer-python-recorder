#!/usr/bin/env python3
"""Fail unless this interpreter is Python ``<major>.<minor>`` as given.

``just check-python <version>`` runs this through ``uv run``, so the
interpreter it inspects is the one ``uv run`` resolves -- the same resolution
every test recipe goes through. A CI job that names a Python version calls it
before its first test step; see the ``venv`` recipe in the Justfile for the
defect it guards against (a version given to ``uv sync -p`` and dropped by the
next ``uv run``).
"""

from __future__ import annotations

import re
import sys


def check(want: str, version_info: tuple[int, ...]) -> str | None:
    """Return an error message, or ``None`` when ``version_info`` is ``want``."""
    if not re.fullmatch(r"\d+\.\d+", want):
        return f"error: expected a <major>.<minor> version such as 3.13, got {want!r}"
    got = f"{version_info[0]}.{version_info[1]}"
    if got != want:
        return (
            f"error: expected Python {want}, but `uv run` resolves Python {got}. "
            "Select the interpreter with UV_PYTHON for every step, not with -p "
            "on one uv command."
        )
    return None


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: check_python_version.py <major>.<minor>", file=sys.stderr)
        return 2
    print(f"uv run resolves Python {sys.version.split()[0]} at {sys.executable}")
    error = check(argv[1], tuple(sys.version_info))
    if error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
