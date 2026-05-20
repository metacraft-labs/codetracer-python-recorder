# codetracer-pure-python-recorder

Pure-Python reference implementation of the CodeTracer Python recorder.
**Legacy JSON trace output by design.**

## Why a pure-Python version exists

The production recorder lives in the sibling project
[`codetracer-python-recorder/`](../codetracer-python-recorder/) — a
Rust extension built with PyO3 + maturin. It emits CTFS v3 binary
trace bundles per `codetracer-specs/Recorder-CLI-Conventions.md` §4.

This package deliberately stays on the older JSON shape and is the
cross-validation oracle that keeps the native recorder honest.

The repository's test suite runs the same test programs through
**both** recorders:

1. The pure-Python recorder writes its JSON trace files directly.
2. The native recorder writes `<prog>.ct`. The integration tests
   (see
   [`codetracer-python-recorder/tests/python/test_cli_integration.py`](../codetracer-python-recorder/tests/python/test_cli_integration.py))
   shell out to `ct print --json-events` (from
   [`codetracer-trace-format-nim`](https://github.com/metacraft-labs/codetracer-trace-format-nim))
   to convert the CTFS bundle into a JSON event stream, then compare
   against assertions and shared fixtures.

That symmetry is the whole point: any behaviour change in the native
recorder is caught by structural divergence from the pure reference.
If both recorders quietly drifted in lockstep, the test suite would
lose its independent oracle.

## When to modify this recorder

- **Trace-shape change** (new event kind, new field, semantic
  adjustment): change the pure recorder first to pin down the
  intended shape, update fixtures, then mirror the change in the
  native recorder until tests are green. The pure recorder is treated
  as the canonical specification of the recorded behaviour.
- **Bug fix that only affects this recorder**: fix it, update
  fixtures if needed, and — critically — verify the native recorder
  did not silently rely on the same buggy shape.
- **Fixture regeneration**: whenever the JSON output changes, the
  test-side conversion path (`ct print --json-events` plus any
  normalisation done in
  [`codetracer-python-recorder/tests/python/test_cli_integration.py`](../codetracer-python-recorder/tests/python/test_cli_integration.py))
  must move in lockstep.

## What NOT to do

- **Do not migrate this package to CTFS v3.** That would defeat the
  cross-validation oracle and silently weaken the test suite. If you
  need CTFS output from Python, use the native recorder at
  [`../codetracer-python-recorder/`](../codetracer-python-recorder/).
- **Do not rename or reshape JSON fields without updating fixtures
  and the test-side ct-print path together.** They are coupled on
  purpose; the coupling is what gives the test suite its independent
  oracle.
- **Do not optimise this recorder for production throughput.** It is
  a reference implementation. Clarity beats speed here; speed is the
  native recorder's job.

## Audience

Reading this six months from now and wondering why this package
still exists in the CTFS era? It exists so the test suite has two
independent implementations to compare. That redundancy is the
design.

## CLI

```bash
codetracer-record <path to python file>
# Produces several trace JSON files in the current directory, or in
# $CODETRACER_DB_TRACE_PATH when that env var is set.
```

During development you can also run the entry script directly:

```bash
python src/trace.py <path to python file>
```

## See also

- [`../codetracer-python-recorder/`](../codetracer-python-recorder/) —
  production native recorder (CTFS v3, PyO3 + maturin).
- [`../codetracer-python-recorder/tests/python/test_cli_integration.py`](../codetracer-python-recorder/tests/python/test_cli_integration.py)
  — cross-recorder integration tests; the `ct print --json-events`
  invocation there documents how the native recorder's CTFS output
  is brought back into a JSON shape that can be compared against the
  pure recorder.
- [`../CLAUDE.md`](../CLAUDE.md) — repo-level notes including the
  rationale for keeping both recorders side by side.
