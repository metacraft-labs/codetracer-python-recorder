# codetracer-pure-python-recorder: notes for agents

**This package is a test oracle. It is not a production recorder.**

- It writes JSON (`trace.json`, `trace_paths.json`, `trace_metadata.json`)
  and nothing else. **CodeTracer cannot open that output** and must not be
  taught to: it is not a recording. CodeTracer opens only CTFS `.ct`
  recordings, written by the production recorder in
  `../codetracer-python-recorder/`.
- Its purpose is to be an independent second implementation the test suite
  can compare the production recorder against.

## The testing protocol

1. Run a program through this recorder -> `trace.json`.
2. Run the same program through the production recorder -> `<program>.ct`.
3. Convert the `.ct` with `ct print` (`ct-print --full` from
   `codetracer-trace-format-nim`; set `CT_PRINT` to override its location).
4. Project both onto the facts both recorders must agree on (functions
   called, lines executed, local values, return values) and assert equality.

This is implemented in
`../codetracer-python-recorder/tests/python/test_pure_oracle.py` (run by
`just py-test`). The comparison must never be vacuous: the test asserts that
both sides recorded calls, steps and values before comparing them. Keep it
that way.

`tests/test_trace.py` here only checks this recorder against its own golden
fixtures in `tests/fixtures/`.

## Rules

- Do not migrate this recorder to CTFS, and do not add a CTFS or `.ct`
  output. That would remove the independent oracle.
- Do not make CodeTracer (db-backend, CLI, frontend) read this JSON.
- Do not add a JSON output mode to the production recorder to "match" this
  one; the production recorder writes CTFS only, and the comparison goes
  through `ct print`.
- When the intended trace shape changes, change this recorder and its
  fixtures, then make the production recorder agree, keeping
  `test_pure_oracle.py` green.
- Clarity over speed: this is a reference implementation.
