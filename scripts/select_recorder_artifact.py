#!/usr/bin/env python3
"""Select the appropriate recorder build artifact for a given interpreter."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path
from typing import Literal


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--wheel-dir",
        type=Path,
        required=True,
        help="Directory containing maturin build outputs (wheels and/or sdists).",
    )
    parser.add_argument(
        "--mode",
        choices=("wheel", "sdist"),
        default="wheel",
        help="Prefer a wheel for the current interpreter or fallback to the source distribution.",
    )
    parser.add_argument(
        "--platform",
        choices=("auto", "linux", "macos", "windows", "any"),
        default="auto",
        help="Restrict wheel selection to the given platform; defaults to the current platform.",
    )
    return parser.parse_args()


def _normalize_platform(value: str) -> Literal["linux", "macos", "windows", "any"]:
    if value == "auto":
        if sys.platform.startswith("linux"):
            return "linux"
        if sys.platform == "darwin":
            return "macos"
        if sys.platform.startswith("win"):
            return "windows"
        return "any"
    return value  # type: ignore[return-value]


def _is_wheel_compatible(path: Path, platform: str) -> bool:
    name = path.name
    if platform == "any":
        return True
    if platform == "linux":
        return "manylinux" in name or "linux" in name
    if platform == "macos":
        return "macosx" in name or "universal2" in name
    if platform == "windows":
        return "win" in name
    return False


def choose_artifact(wheel_dir: Path, mode: str, platform: str) -> Path:
    platform = _normalize_platform(platform)
    candidates: list[Path] = []
    if mode == "wheel":
        abi = f"cp{sys.version_info.major}{sys.version_info.minor}"
        pattern = f"codetracer_python_recorder-*-{abi}-{abi}-*.whl"
        candidates = [
            path
            for path in sorted(wheel_dir.glob(pattern))
            if _is_wheel_compatible(path, platform)
        ]
    if not candidates:
        candidates = sorted(wheel_dir.glob("codetracer_python_recorder-*.tar.gz"))
    if not candidates:
        raise FileNotFoundError(f"No build artefacts found in {wheel_dir}")
    return candidates[-1]


def main() -> int:
    args = parse_args()
    artifact = choose_artifact(args.wheel_dir, args.mode, args.platform)
    sys.stdout.write(str(artifact))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
