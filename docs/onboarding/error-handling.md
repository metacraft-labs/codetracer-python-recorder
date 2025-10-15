# Recorder Error Handling Onboarding

This note aligns new contributors and downstream consumers on the structured error work. Keep it close when you wire the recorder into tools or review patches that touch failure paths.

## Error classes at a glance
- `RecorderError` is the base class. Subclasses are `UsageError`, `EnvironmentError`, `TargetError`, and `InternalError`.
- Every instance exposes `code` (an `ERR_*` string), `kind` (matches the class), and a `context` dict with string keys.
- Codes stay stable. Add new codes instead of recycling strings.
- The Rust layer also attaches a source error when possible; Python reprs show it as `caused by ...`.

## Python API quick start
```python
from codetracer_python_recorder import RecorderError, TargetError, start, stop

try:
    session = start("/tmp/trace", format="json")
except RecorderError as err:
    print(f"Recorder failed: {err.code}")
    for key, value in err.context.items():
        print(f"  {key}: {value}")
else:
    try:
        ...  # run traced work here
    finally:
        session.flush()
        stop()
```
- Catch `RecorderError` when you want a single guard. Catch subclasses when you care about `UsageError` vs `TargetError`.
- Calling `start` twice raises `RuntimeError` from a thin Python guard. Everything after the guard uses `RecorderError`.

## CLI workflow and JSON trailers
- Run `python -m codetracer_python_recorder --codetracer-format=json app.py` to trace a script.
- Exit codes: `0` for success, script exit code when the script stops itself, `1` when a `RecorderError` escapes startup/shutdown, `2` on CLI misuse.
- Pass `--codetracer-json-errors` (or `configure_policy(json_errors=True)`) to mirror each failure as a one-line JSON object on stderr.
- JSON fields: `run_id`, optional `trace_id`, `error_code`, `error_kind`, `message`, `context`.

## Migration checklist for existing clients
1. Replace `RuntimeError` / string matching with `RecorderError` + `err.code` checks.
2. Forward policy options through `configure_policy` (or `policy_snapshot`) instead of reinventing env parsing.
3. Expect structured log lines on stderr. Parse JSON and read the `error_code` field.
4. Opt in to JSON trailers when you need machine-readable failure signals.
5. Keep CLI wrappers short. Avoid reformatting the recorder message; attach extra context alongside it.

## Assertion rules for recorder code
- Use `ensure_usage!`, `ensure_env!`, or `ensure_internal!` when translating invariants into classified failures.
- Reach for `bug!` when you hit a state that should never happen in production.
- Reserve `assert!` and `debug_assert!` for tests or temporary invariants. If you need a dev-only guard, combine `debug_assert!` with the matching `ensure_*` call so production still fails cleanly.
- Never reintroduce `.unwrap()` inside the recorder crate without extending the allowlist. Use the macros instead.

## Tooling guardrails
- Run `just lint` before sending a patch. It runs Clippy with `-D clippy::panic` and our unwrap scanner.
- Run `just test` to exercise Rust (nextest) and Python suites. Failure injections cover permission errors, target crashes, and panic paths.
- Enable the `integration-test` cargo feature when you add new Python surface tests so the Rust hooks are active.
- When in doubt, add a regression test alongside the docs. The plan treats docs plus tests as the definition of done.

## Need help?
- Check `design-docs/error-handling-implementation-plan.md` for context and open questions.
- Ping the error-handling working thread if a new code or policy toggle seems missing. The goal is to keep `RecorderError` exhaustive, not to fork ad hoc enums in downstream tools.
