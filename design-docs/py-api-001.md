# Python sys.monitoring Tracer API

## Overview
This document describes the user-facing Python API for the `codetracer` module built on top of `runtime_tracing` and `sys.monitoring`.  The API exposes a minimal surface for starting and stopping traces, managing trace sessions, and integrating tracing into scripts or test suites.

## Module `codetracer`

### Constants
- `DEFAULT_FORMAT: str = "binary"`
- `TRACE_BINARY: str = "binary"`
- `TRACE_JSON: str = "json"`

### Session Management
- Start a global trace; returns a `TraceSession`.
  ```py
  def start(path: str | os.PathLike, *, format: str = DEFAULT_FORMAT,
            capture_values: bool = True, source_roots: Iterable[str | os.PathLike] | None = None) -> TraceSession
  ```
- Stop the active trace if any.
  ```py
  def stop() -> None
  ```
- Query whether tracing is active.
  ```py
  def is_tracing() -> bool
  ```
- Context manager helper for scoped tracing.
  ```py
  @contextlib.contextmanager
  def trace(path: str | os.PathLike, *, format: str = DEFAULT_FORMAT,
            capture_values: bool = True, source_roots: Iterable[str | os.PathLike] | None = None):
      ...
  ```
- Flush buffered data to disk without ending the session.
  ```py
  def flush() -> None
  ```

## Class `TraceSession`
Represents a live tracing session returned by `start()` and used by the context manager.

```py
class TraceSession:
    path: pathlib.Path
    format: str

    def stop(self) -> None: ...
    def flush(self) -> None: ...
    def __enter__(self) -> TraceSession: ...
    def __exit__(self, exc_type, exc, tb) -> None: ...
```

## Environment Integration
- Auto-start tracing when `CODETRACER_TRACE` is set; the value is interpreted as the output path.
- When `CODETRACER_FORMAT` is provided, it overrides the default output format.
- `CODETRACER_CAPTURE_VALUES` toggles value recording.

## Usage Example
```py
import codetracer

with codetracer.trace("trace.bin"):
    run_application()
```
