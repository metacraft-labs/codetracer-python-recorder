# General Issues

## ISSUE-001
### Description
We need to record function arguments when calling a function

We have a function `encode_value` which is used to convert Python objects to value records. We need to use this function to encode the function arguments. To do that we should modify the `on_py_start` hook to load the current frame and to read the function arguments from it.

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

### Status
Partially done

Implemented varargs (`*args`), keyword-only, and kwargs (`**kwargs`) capture.
Positional-only parameters are read (`co_posonlyargcount`) but not yet included
in the positional slice, so they are currently omitted. Follow-up: include
`posonly + co_argcount` when selecting positional names.

## ISSUE-005
### Description
Include positional-only parameters in argument capture. The current logic uses
only `co_argcount` for the positional slice, which excludes positional-only
arguments (PEP 570). As a result, names before the `/` in a signature like
`def f(p, /, q, *args, r, **kwargs)` are dropped.

### Status
Not started

## ISSUE-003
### Description
Avoid defensive fallback in argument capture. The current change swallows
failures to access the frame/locals and proceeds with empty `args`. Per
`rules/source-code.md` ("Avoid defensive programming"), we should fail fast when
encountering such edge cases, or make soft-fail behavior explicitly opt-in.

### Status
Not started

## ISSUE-004
### Description
Stabilize string value encoding for arguments and tighten tests. The new test
accepts either `String` or `Raw` kinds for the `'x'` argument, which can hide
regressions. We should standardize encoding of `str` as `String` (or document
when `Raw` is expected) and update tests to assert the exact kind.

### Status
Not started
