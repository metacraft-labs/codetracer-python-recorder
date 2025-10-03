# Tracer Test Plan (Quick List)

## Setup
- Use a temp directory per test.
- Reset global tracer state between runs.

## Startup + shutdown
- Tool id is reused after first acquisition.
- Callbacks register/unregister cleanly.
- Stopping the tracer leaves no stray events; `os._exit` leaves the file truncated but valid.

## Event coverage
- `PY_START`/`PY_RETURN` pair for a simple function.
- Generators hit `PY_RESUME` and `PY_YIELD`.
- Exceptions trigger `PY_THROW`, `RERAISE`, and `EXCEPTION_HANDLED` as expected.
- Calls record frame ids correctly, including recursion and decorated functions.
- Branching code emits `LINE`, `BRANCH`, and `JUMP` events with the right lines.
- C boundary logs `C_CALL`/`C_RETURN` and `C_RAISE` for failures.

## Value capture
- Basic scalars and collections encode round-trip.
- Recursive structures stop at the cycle guard.
- Unhandled objects fall back to a stable `repr` string.

## Metadata
- Source files are copied once with hashes.
- Process metadata (Python version, platform) is present.

## Edge checks
- Invalid manual registration raises `ValueError`.
- Re-tracing after stop is a no-op.
- Oversized strings get truncated with a marker.
- Stress test: 10^6 loop iterations finish within budget and threads keep ids distinct.
