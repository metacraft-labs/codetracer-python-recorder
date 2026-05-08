"""Environment-driven trace auto-start helper.

Auto-start is the library-mode entry point: setting ``CODETRACER_TRACE``
to a directory before importing the recorder triggers tracing without
running the CLI.  Per ``codetracer-specs/Recorder-CLI-Conventions.md``
§4 the recorder always writes CTFS, so this module no longer reads any
``CODETRACER_FORMAT`` environment variable; the format is hard-pinned
to CTFS.

The CLI-side env vars (``CODETRACER_PYTHON_RECORDER_OUT_DIR`` and
``CODETRACER_PYTHON_RECORDER_DISABLED``) are unrelated to this auto-start
path and live in ``cli.py``.
"""
from __future__ import annotations

import logging
import os
from typing import Iterable

from .formats import DEFAULT_FORMAT

ENV_TRACE_PATH = "CODETRACER_TRACE"
ENV_TRACE_FILTER = "CODETRACER_TRACE_FILTER"

log = logging.getLogger(__name__)


def auto_start_from_env() -> None:
    """Start tracing automatically when the relevant environment variables are set.

    Recognises:

    * ``CODETRACER_TRACE`` — destination directory for the trace.  When
      unset, this function is a no-op.
    * ``CODETRACER_TRACE_FILTER`` — optional filter chain (``::``-
      separated paths).

    The output format is hard-pinned to CTFS per the CTFS-only contract;
    no ``CODETRACER_FORMAT`` lookup occurs.
    """
    path = os.getenv(ENV_TRACE_PATH)
    if not path:
        return
    filter_spec = os.getenv(ENV_TRACE_FILTER)

    # Delay import to avoid boot-time circular dependencies.
    from . import session
    from .codetracer_python_recorder import configure_policy_from_env as _configure_policy_from_env

    _configure_policy_from_env()

    if session.is_tracing():
        log.debug("codetracer auto-start skipped: tracing already active")
        return

    log.debug(
        "codetracer auto-start triggered",
        extra={"trace_path": path, "format": DEFAULT_FORMAT, "trace_filter": filter_spec},
    )
    session.start(path, format=DEFAULT_FORMAT, trace_filter=filter_spec)


__all__: Iterable[str] = (
    "ENV_TRACE_PATH",
    "ENV_TRACE_FILTER",
    "auto_start_from_env",
)
