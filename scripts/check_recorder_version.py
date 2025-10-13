#!/usr/bin/env python3
"""Verify codetracer recorder version parity across Python and Rust manifests."""

from __future__ import annotations

import sys
from pathlib import Path

try:  # Python 3.11+
    import tomllib  # type: ignore[attr-defined]
except ModuleNotFoundError:  # pragma: no cover - safety net for older interpreters
    tomllib = None  # type: ignore[assignment]

REPO_ROOT = Path(__file__).resolve().parents[1]
PYPROJECT = REPO_ROOT / "codetracer-python-recorder" / "pyproject.toml"
CARGO = REPO_ROOT / "codetracer-python-recorder" / "Cargo.toml"


def _read_version(path: Path, section: str) -> str:
    text = path.read_text(encoding="utf-8")
    if tomllib is not None:
        data = tomllib.loads(text)
        section_data = data.get(section)
        if not isinstance(section_data, dict):
            raise KeyError(f"Missing section [{section}] in {path}")
        version = section_data.get("version")
        if not isinstance(version, str):
            raise KeyError(f"Missing 'version' in [{section}] of {path}")
        return version

    # Minimal parser fallback for environments without tomllib/tomli.
    target_header = f"[{section}]"
    in_section = False
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            in_section = line == target_header
            continue
        if in_section and line.startswith("version"):
            _, _, value = line.partition("=")
            version = value.strip().strip('"')
            if version:
                return version
    raise KeyError(f"Could not locate version in [{section}] of {path}")


def main() -> int:
    python_version = _read_version(PYPROJECT, "project")
    rust_version = _read_version(CARGO, "package")
    if python_version != rust_version:
        sys.stderr.write(
            "Version mismatch detected:\n"
            f"  pyproject.toml -> {python_version}\n"
            f"  Cargo.toml     -> {rust_version}\n"
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
