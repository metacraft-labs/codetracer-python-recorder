"""Thin wrapper for running the recorder CLI via ``python -m``."""
from __future__ import annotations

from .cli import main


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
