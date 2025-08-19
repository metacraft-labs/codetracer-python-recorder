"""High-level tracing API built on a Rust backend.

This module exposes a minimal interface for starting and stopping
runtime traces. The heavy lifting is delegated to the
`codetracer_python_recorder` Rust extension which will eventually hook
into `runtime_tracing` and `sys.monitoring`.  For now the Rust side only
maintains placeholder state and performs no actual tracing.
"""

from .api import *

__all__ = api.__all__

