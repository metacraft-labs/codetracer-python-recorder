"""High-level tracing API backed by the Rust runtime tracer.

`codetracer_python_recorder` installs a `sys.monitoring` tool that streams
events into the shared `runtime_tracing` format. Line events now include a
full locals snapshot for every Python scope (functions, classes, module
top-level execution, comprehensions, and generators). Imported modules and
``__builtins__`` are filtered when present; global tracking remains a
follow-up (ISSUE-013).
"""

from . import api as _api
from .api import *  # re-export public API symbols

__all__ = _api.__all__
