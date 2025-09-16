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
Done

Implemented for positional (and pos-or-keyword) arguments on function entry
using `sys._getframe(0)` and `co_varnames[:co_argcount]`, with counting fixed to
use `co_argcount` directly (includes positional-only; avoids double-counting).
Values are encoded via `encode_value` and attached to the `Call` event. Tests
validate correct presence and values. Varargs/kwargs remain covered by
ISSUE-002.


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
Done

All argument kinds are captured on function entry, including kwargs with
structured encoding. Varargs are preserved as `Tuple` (per CPython), and
`**kwargs` are encoded as a `Sequence` of 2-element `Tuple`s `(key, value)`
with string keys, enabling lossless downstream analysis. The updated test
`test_all_argument_kinds_recorded_on_py_start` verifies the behavior.

Note: While the original Definition of Done referenced a mapping value kind,
the implementation follows the proposed approach in ISSUE-008 to represent
kwargs as a sequence of tuples using existing value kinds.

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

Implemented by selecting positional names from `co_varnames` using
`co_argcount` directly (which already includes positional-only per CPython 3.8+).
This prevents double-counting and keeps indexing stable. Tests in
`test_all_argument_kinds_recorded_on_py_start` assert presence of the
positional-only parameter `p` and pass.

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
Done

Implemented soft-stop on first callback error in `callback_py_start`:
on error, the tracer finishes writers, unregisters callbacks for the
configured mask, sets events to `NO_EVENTS`, clears the registry, and
records `global.mask = NO_EVENTS`. The original error is propagated to
Python, and subsequent `PY_START` events are not delivered. This keeps the
module-level `ACTIVE` flag unchanged until `stop_tracing()`, making the
shutdown idempotent. The test `tests/test_fail_fast_on_py_start.py`
exercises the behavior by re-running the program after the initial failure.

## ISSUE-008
### Description
Provide structured encoding for kwargs (`**kwargs`) on function entry. The
current backend encodes kwargs as `Raw` text because the `runtime_tracing`
format lacks a mapping value. Introduce a mapping representation so kwargs can
be recorded losslessly with key/value structure and recursively encoded values.

### Definition of Done
- `runtime_tracing` supports a mapping value kind (e.g., `Map` with string keys).
- `RuntimeTracer::encode_value` encodes Python `dict` to the mapping kind with
  recursively encoded values; key type restricted to `str` (non-`str` keys may
  be stringified or rejected, behavior documented).
- `on_py_start` records `**kwargs` using the new mapping encoding.
- Tests verify kwargs structure and values; large and nested kwargs are covered.

### Proposed solution
- We can represent our `Map` as a sequenced of tuples. This way we can use the current value record types to encode dictionaries.
- In the Python recorder, downcast to `dict` and iterate items, encoding values
  recursively; keep behavior minimal and fail fast on unexpected key types per
  repo rules (no defensive fallbacks).

### Dependent issues
- Blocks completion of ISSUE-002

### Status
Done

Implemented structured kwargs encoding in the Rust tracer by representing
Python `dict` as a `Sequence` of `(key, value)` `Tuple`s, with keys encoded as
`String` when possible. Tests in
`codetracer-python-recorder/test/test_monitoring_events.py` validate that
kwargs are recorded structurally. This fulfills the goal without introducing a
new mapping value kind, per the proposed solution.


## ISSUE-009
### Description
Unify list/sequence `lang_type` naming across recorders. The Rust tracer now
emits `TypeKind::Seq` with name "List" for Python `list`, while the
pure-Python recorder uses "Array". This divergence can fragment the trace
schema and complicate downstream consumers.

### Definition of Done
- Both recorders emit the same `lang_type` for Python list values.
- Fixtures and docs/spec are updated to reflect the chosen term.
- Cross-recorder tests pass with consistent types.

### Proposed solution
- We will use "List" in order to match existing Python nomenclature

### Status
Low priority. We won't work on this unless it blocks another issue.


## ISSUE-010
### Description
Clarify scope of dict structural encoding and key typing. The current change
encodes any Python `dict` as a `Sequence` of `(key, value)` tuples and falls
back to generic encoding for non-string keys. Repo rules favor fail-fast over
defensive fallbacks, and ISSUE-008 focused specifically on `**kwargs`.

### Definition of Done
- Decide whether structural dict encoding should apply only to kwargs or to all
  dict values; document the choice.
- If limited to kwargs, restrict structured encoding to kwargs capture sites.
- If applied generally, define behavior for non-string keys (e.g., fail fast)
  and add tests for nested and non-string-key dicts.

### Proposed solution
- Prefer failing fast on non-string keys in contexts representing kwargs; if
  general dict encoding is retained, update the spec and tests and remove the
  defensive fallback for key encoding.

### Status
Low priority. We won't work on this until a user reports that it causes issues.
