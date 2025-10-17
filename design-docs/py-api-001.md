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
            start_on_enter: str | os.PathLike | None = None) -> TraceSession
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
  def trace(path: str | os.PathLike, *, format: str = DEFAULT_FORMAT):
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

### Start Behavior
- `start_on_enter`: Optional path; when provided, tracing starts only after execution first enters this file (useful to avoid interpreter/import noise when launching via CLI).

### Output Location
- `path` is a directory. The tracer writes three files inside it:
  - `trace.json` when `format == "json"` or `trace.bin` when `format == "binary"`
  - `trace_metadata.json`
  - `trace_paths.json`

## Environment Integration
- Auto-start tracing when `CODETRACER_TRACE` is set; the value is interpreted as the output directory.
- When `CODETRACER_FORMAT` is provided, it overrides the default output format.
- Accept `CODETRACER_TRACE_FILTER` with either `::`-separated paths or multiple
  entries (mirroring the CLI). The env-driven chain is appended after any
  discovered project default `.codetracer/trace-filter.toml`, allowing overrides
  to refine or replace default rules.
- Even when no env/CLI filters are provided, prepend the bundled `builtin-default`
  filter so a baseline redaction/stdlib skip policy always applies.

## Usage Example
```py
import codetracer
from pathlib import Path

out_dir = Path("./traces/run-001")
with codetracer.trace(out_dir, format=codetracer.TRACE_JSON):
    run_application()
```
