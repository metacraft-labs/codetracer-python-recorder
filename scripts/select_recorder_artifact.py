#!/usr/bin/env python3
"""Select the appropriate recorder build artifact for a given interpreter."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


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
    return parser.parse_args()


def choose_artifact(wheel_dir: Path, mode: str) -> Path:
    candidates = []
    if mode == "wheel":
        abi = f"cp{sys.version_info.major}{sys.version_info.minor}"
        pattern = f"codetracer_python_recorder-*-{abi}-{abi}-*.whl"
        candidates = sorted(wheel_dir.glob(pattern))
    if not candidates:
        candidates = sorted(wheel_dir.glob("codetracer_python_recorder-*.tar.gz"))
    if not candidates:
        raise FileNotFoundError(f"No build artefacts found in {wheel_dir}")
    return candidates[-1]


def main() -> int:
    args = parse_args()
    artifact = choose_artifact(args.wheel_dir, args.mode)
    sys.stdout.write(str(artifact))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
