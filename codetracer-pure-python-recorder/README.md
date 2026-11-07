# codetracer-pure-python-recorder

**This is a test oracle, not a production recorder.**

It is a small pure-Python implementation of the CodeTracer Python
recorder that writes its trace as plain JSON (`trace.json`,
`trace_paths.json`, `trace_metadata.json`). Its only job is to give the
test suite an independent second opinion on what the production
recorder should have recorded.

> **CodeTracer cannot open this recorder's output.** The JSON files are
> not a recording. CodeTracer only opens CTFS `.ct` recordings, which
> are written by the production recorder in
> [`../codetracer-python-recorder/`](../codetracer-python-recorder/).
> If you want to debug a Python program in CodeTracer, use that one.

## The testing protocol

1. Run a program through this recorder. It writes `trace.json`.
2. Run the **same** program through the production recorder. It writes
   a CTFS `.ct` recording.
3. Convert the `.ct` recording to JSON with `ct print` (the
   `ct-print --full` decoder from `codetracer-trace-format-nim`, which
   is what `ct print` runs).
4. Project both JSON documents onto the facts both recorders are meant
   to agree on (which functions were called, which lines ran, what the
   local variables and return values were) and assert that the two
   projections are equal.

The test that does this is
[`../codetracer-python-recorder/tests/python/test_pure_oracle.py`](../codetracer-python-recorder/tests/python/test_pure_oracle.py),
run by `just py-test` and `just test`. It refuses to pass on an empty
comparison: both sides must record calls, steps and values before the
streams are compared.

The tests in [`tests/`](tests/) only check this recorder against its
own golden fixtures; they say nothing about the production recorder.

That symmetry is the whole point: a behaviour change in the production
recorder shows up as a divergence from this independent
implementation. If both recorders drifted in lockstep, the test suite
would lose its oracle.

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
  projection in
  [`codetracer-python-recorder/tests/python/test_pure_oracle.py`](../codetracer-python-recorder/tests/python/test_pure_oracle.py)
  must move in lockstep.

## What NOT to do

- **Do not migrate this package to CTFS v3.** That would defeat the
  cross-validation oracle and silently weaken the test suite. If you
  need CTFS output from Python, use the native recorder at
  [`../codetracer-python-recorder/`](../codetracer-python-recorder/).
- **Do not rename or reshape JSON fields without updating fixtures
  and the oracle comparison test together.** They are coupled on
  purpose; the coupling is what gives the test suite its independent
  oracle.
- **Do not optimise this recorder for production throughput.** It is
  a reference implementation. Clarity beats speed here; speed is the
  native recorder's job.

## Audience

Reading this six months from now and wondering why this package
still exists in the CTFS era? It exists so the test suite has two
independent implementations to compare. That redundancy is the
design. It is not a fallback recorder, it is not a way to record
programs for debugging, and CodeTracer will refuse to open what it
writes.

## CLI

```bash
codetracer-record <path to python file>
# Writes trace.json, trace_paths.json and trace_metadata.json into the
# current directory. These are test-oracle output, not a recording:
# CodeTracer cannot open them.
```

During development you can also run the entry script directly:

```bash
python src/trace.py <path to python file>
```

## See also

- [`../codetracer-python-recorder/`](../codetracer-python-recorder/) —
  production native recorder (CTFS v3, PyO3 + maturin).
- [`../codetracer-python-recorder/tests/python/test_pure_oracle.py`](../codetracer-python-recorder/tests/python/test_pure_oracle.py)
  — the oracle comparison: records each program with both recorders,
  converts the `.ct` recording with `ct print`, and compares.
- [`AGENTS.md`](AGENTS.md) — the rules for agents working here.
- [`../AGENTS.md`](../AGENTS.md) — repo-level notes including the
  rationale for keeping both recorders side by side.
