# General Issues

## ISSUE-001
### Description
We need to record function arguments when calling a function

We have a function `encode_value` which is used to convert Python objects to value records. We need to use this function to encode the function arguments. To do that we should modify the `on_py_start` hook to load the current frame and to read the function arguments from it.

### Definition of Done
- Arguments for positional and pos-or-keyword parameters are recorded on function entry using the current frame's locals.
- Values are encoded via `encode_value` and attached to the `Call` event payload.
- A unit test asserts that multiple positional arguments (e.g. `a`, `b`) are present with correct encoded values.
- Varargs/kwargs and positional-only coverage are tracked in separate issues (see ISSUE-002, ISSUE-005).

### Status
Partially done

Implemented for positional (and pos-or-keyword) arguments on function entry using
`sys._getframe(0)` and `co_varnames[:co_argcount]`. Values are encoded via
`encode_value` and attached to the `Call` event. A test validates two arguments
(`a`, `b`) are present with correct values.

Out of scope (follow-ups needed): varargs (`*args`) and keyword-only/kwargs
(`**kwargs`). See ISSUE-002.


# Issues Breaking Declared Relations

This document lists concrete mismatches that cause the relations in `relations.md` to fail.

It should be structured like so:
```md
## REL-001
### ISSUE-001-001
#### Description
Blah blah blah
#### Proposed solution
Blah blah bleh

### ISSUE-001-002
...

## REL-002
...
```

## ISSUE-002
### Description
Capture all Python argument kinds on function entry: positional-only,
pos-or-kw, keyword-only, plus varargs (`*args`) and kwargs (`**kwargs`). Extend
the current implementation that uses `co_argcount` and `co_varnames` to also
leverage `co_posonlyargcount` and `co_kwonlyargcount`, and detect varargs/kwargs
via code flags. Encode `*args` as a list value and `**kwargs` as a mapping value
to preserve structure.

### Definition of Done
- All argument kinds are captured on function entry: positional-only, pos-or-keyword, keyword-only, varargs (`*args`), and kwargs (`**kwargs`).
- `*args` is encoded as a list value; `**kwargs` is encoded as a mapping value.
- Positional-only and keyword-only parameters are included using `co_posonlyargcount` and `co_kwonlyargcount`.
- Comprehensive tests cover each argument kind and validate the encoded structure and values.

### Status
Partially done

Implemented varargs (`*args`), keyword-only, and kwargs (`**kwargs`) capture.
Positional-only parameters are now included in the positional slice via
`co_posonlyargcount + co_argcount` (see ISSUE-005). Remaining gap: structured
encoding for `*args`/`**kwargs` per Definition of Done (currently accepted as
backend-dependent; tests allow `Raw`).

## ISSUE-005
### Description
Include positional-only parameters in argument capture. The current logic uses
only `co_argcount` for the positional slice, which excludes positional-only
arguments (PEP 570). As a result, names before the `/` in a signature like
`def f(p, /, q, *args, r, **kwargs)` are dropped.

### Definition of Done
- Positional-only parameters are included in the captured argument set.
- The selection of positional names accounts for `co_posonlyargcount` in addition to `co_argcount`.
- Tests add a function with positional-only parameters and assert their presence and correct encoding.

### Status
Done

Implemented by selecting positional names from `co_varnames` with
`co_posonlyargcount + co_argcount`. Tests in `test_all_argument_kinds_recorded_on_py_start`
assert presence of the positional-only parameter `p` and pass.

## ISSUE-003
### Description
Avoid defensive fallback in argument capture. The current change swallows
failures to access the frame/locals and proceeds with empty `args`. Per
`rules/source-code.md` ("Avoid defensive programming"), we should fail fast when
encountering such edge cases.

### Definition of Done
- Silent fallbacks that return empty arguments on failure are removed.
- The recorder raises a clear, actionable error when it cannot access frame/locals.
- Tests verify the fail-fast path.

### Status
Done

`RuntimeTracer::on_py_start` now returns `PyResult<()>` and raises a
`RuntimeError` when frame/locals access fails; `callback_py_start` propagates
the error to Python. A pytest (`tests/test_fail_fast_on_py_start.py`) asserts
the fail-fast behavior by monkeypatching `sys._getframe` to raise.

## ISSUE-004
### Description
Stabilize string value encoding for arguments and tighten tests. The new test
accepts either `String` or `Raw` kinds for the `'x'` argument, which can hide
regressions. We should standardize encoding of `str` as `String` (or document
when `Raw` is expected) and update tests to assert the exact kind.

### Definition of Done
- String values are consistently encoded as `String` (or the expected canonical kind), with any exceptions explicitly documented.
- Tests assert the exact kind for `str` arguments and fail if an unexpected kind (e.g., `Raw`) is produced.
- Documentation clarifies encoding rules for string-like types to avoid ambiguity in future changes.

### Status
Done

Stricter tests now assert `str` values are encoded as `String` with the exact text payload, and runtime docs clarify canonical encoding. No runtime logic change was required since `encode_value` already produced `String` for Python `str`.

## ISSUE-006
### Description
Accidental check-in of Cargo cache/artifact files under `codetracer-python-recorder/.cargo/**` (e.g., `registry/CACHEDIR.TAG`, `.package-cache`). These are build/cache directories and should be excluded from version control.

### Definition of Done
- Add ignore rules to exclude Cargo cache directories (e.g., `.cargo/**`, `target/**`) from version control.
- Remove already-checked-in cache files from the repository.
- Verify the working tree is clean after a clean build; no cache artifacts appear as changes.

### Status
Done

## ISSUE-007
### Description
Immediately stop tracing when any monitoring callback raises an error.

Current behavior: `RuntimeTracer::on_py_start` intentionally fails fast when it
cannot capture function arguments (e.g., when `sys._getframe` is unavailable or
patched to raise). The callback error is propagated to Python via
`callback_py_start` (it returns the `PyResult` from `on_py_start`). However, the
tracer remains installed and active after the error. As a result, any further
Python function start (even from exception-handling or printing the exception)
triggers `on_py_start` again, re-raising the same error and interfering with the
program’s own error handling.

This is observable in `codetracer-python-recorder/tests/test_fail_fast_on_py_start.py`:
the test simulates `_getframe` failure, which correctly raises in `on_py_start`,
but `print(e)` inside the test’s `except` block invokes codec machinery that
emits additional `PY_START` events. Those callbacks raise again, causing the test
to fail before reaching its assertions.

### Impact
- Breaks user code paths that attempt to catch and handle exceptions while the
  tracer is active — routine operations like `print(e)` can cascade failures.
- Hard to debug because the original error is masked by subsequent callback
  errors from unrelated modules (e.g., `codecs`).

### Proposed Solution
Fail fast and disable tracing at the first callback error.

Implementation sketch:
- In each callback wrapper (e.g., `callback_py_start`), if the underlying
  tracer method returns `Err`, immediately disable further monitoring before
  returning the error:
  - Set events to `NO_EVENTS` (via `set_events`) to prevent any more callbacks.
  - Unregister all previously registered callbacks for our tool id.
  - Optionally call `finish()` on the tracer to flush/close writers.
  - Option A (hard uninstall): call `uninstall_tracer(py)` to release tool id
    and clear the registry. This fully tears down the tracer. Note that the
    high-level `ACTIVE` flag in `lib.rs` is not updated by `uninstall_tracer`,
    so either:
      - expose an internal “deactivate_from_callback()” in `lib.rs` that clears
        `ACTIVE`, or
      - keep a soft-stop in `tracer.rs` by setting `NO_EVENTS` and unregistering
        callbacks without touching `ACTIVE`, allowing `stop_tracing()` to be a
        no-op later.
  - Ensure reentrancy safety: perform the disable sequence only once (e.g., with
    a guard flag) to avoid nested teardown during callback execution.

Behavioral details:
- The original callback error must still be propagated to Python so the user
  sees the true failure cause, but subsequent code should not receive further
  monitoring callbacks.
- If error occurs before activation gating triggers, the disable sequence should
  still run to avoid repeated failures from unrelated modules importing.

### Definition of Done
- On any callback error (at minimum `on_py_start`, and future callbacks that may
  return `PyResult`), all further monitoring callbacks from this tool are
  disabled immediately within the same GIL context.
- The initial error is propagated unchanged to Python.
- The failing test `test_fail_fast_on_py_start.py` passes: after the first
  failure, `print(e)` does not trigger additional tracer errors.
- Writers are flushed/closed or left in a consistent state (documented), and no
  additional events are recorded after disablement.
- Unit/integration tests cover: error in `on_py_start`, repeated calls after
  disablement are no-ops, and explicit `stop_tracing()` is safe after a
  callback-induced shutdown.

### Status
Not started
