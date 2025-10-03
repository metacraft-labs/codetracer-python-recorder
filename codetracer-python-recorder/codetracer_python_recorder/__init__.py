"""Public tracing surface with structured recorder errors.

Importing this package installs policy defaults, wires the Rust backend,
and exposes helpers to start and stop tracing. Every failure travels
through :class:`RecorderError` or one of its subclasses. Each exception
carries a stable ``code`` string (``ERR_*``), a ``kind`` label, and a
``context`` dict for tooling.
"""

from . import api as _api
from .api import *  # re-export public API symbols
from .auto_start import auto_start_from_env
from .codetracer_python_recorder import (
    EnvironmentError,
    InternalError,
    RecorderError,
    TargetError,
    UsageError,
    configure_policy,
    configure_policy_from_env,
    policy_snapshot,
)

configure_policy_from_env()
auto_start_from_env()

__all__ = (
    *_api.__all__,
    "RecorderError",
    "UsageError",
    "EnvironmentError",
    "TargetError",
    "InternalError",
    "configure_policy",
    "configure_policy_from_env",
    "policy_snapshot",
)
