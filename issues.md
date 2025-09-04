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
