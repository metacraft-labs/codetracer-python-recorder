"""Environment-driven trace auto-start helper."""
from __future__ import annotations

import logging
import os
from typing import Iterable

from .formats import DEFAULT_FORMAT

ENV_TRACE_PATH = "CODETRACER_TRACE"
ENV_TRACE_FORMAT = "CODETRACER_FORMAT"

log = logging.getLogger(__name__)


def auto_start_from_env() -> None:
    """Start tracing automatically when the relevant environment variables are set."""
    path = os.getenv(ENV_TRACE_PATH)
    if not path:
        return

    # Delay import to avoid boot-time circular dependencies.
    from . import session

    if session.is_tracing():
        log.debug("codetracer auto-start skipped: tracing already active")
        return

    fmt = os.getenv(ENV_TRACE_FORMAT, DEFAULT_FORMAT)
    log.debug(
        "codetracer auto-start triggered", extra={"trace_path": path, "format": fmt}
    )
    session.start(path, format=fmt)


__all__: Iterable[str] = (
    "ENV_TRACE_FORMAT",
    "ENV_TRACE_PATH",
    "auto_start_from_env",
)
