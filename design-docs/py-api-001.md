# Python API Cheat Sheet

## Imports
```py
import codetracer
```

## Constants
- `codetracer.TRACE_BINARY` / `codetracer.TRACE_JSON`
- `codetracer.DEFAULT_FORMAT` (defaults to binary)

## Core calls
```py
session = codetracer.start(path, format=codetracer.DEFAULT_FORMAT, start_on_enter=None)
codetracer.stop()
is_active = codetracer.is_tracing()
codetracer.flush()
```

`start_on_enter` (optional path) delays tracing until we enter that file.

## Context manager
```py
with codetracer.trace(path, format=codetracer.TRACE_JSON):
    run_code()
```

## TraceSession object
- Attributes: `path`, `format`
- Methods: `stop()`, `flush()`, context-manager support.

## Files we write
- `trace.bin` or `trace.json`
- `trace_metadata.json`
- `trace_paths.json`

## Environment auto-start
- `CODETRACER_TRACE=/tmp/out` starts tracing on import.
- `CODETRACER_FORMAT=json` overrides the format.
