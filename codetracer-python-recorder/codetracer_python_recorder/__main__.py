"""Entry point for ``python -m codetracer_python_recorder``."""
from __future__ import annotations

from .cli import main


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
