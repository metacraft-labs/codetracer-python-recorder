## ISSUE-011
### Description
Consolidate session/bootstrap error handling while migrating to the central
`RecorderError` façade. Current call sites return raw `PyRuntimeError` strings
without classification:
- `src/session.rs:26`
- `src/session/bootstrap.rs:71`, `:79`, `:92`

### Definition of Done
- Replace the call sites above with `RecorderResult` + structured error codes.
- Python facade work is tracked under ISSUE-014.
- Unit tests cover usage/environment error variants for session startup.

### Status
Open


## ISSUE-012
### Description
Retrofit runtime helpers and value capture to use classified errors. A number
of hotspots still return bare `PyRuntimeError` or rely on `unwrap`/`expect`:
- `src/runtime/mod.rs:125`, `:159`, `:854`
- `src/runtime/frame_inspector.rs:65`, `:85`, `:99`, `:117`, `:122`, `:129`,
  `:134`, `:141`, `:148`
- `src/runtime/value_capture.rs:43`, `:62`
- `src/runtime/value_encoder.rs:70-71`
- `src/runtime/activation.rs:24`

### Definition of Done
- All sites above return `RecorderResult` with stable `ErrorCode`s; unwraps are
  replaced by guarded conversions or `bug!` macros.
- Runtime tracer IO paths adopt the atomic write façade defined in ADR 0004.
- Regression tests cover failure paths (missing locals/globals, encoding
  mismatches).

### Status
Open


## ISSUE-013
### Description
Harden monitoring/FFI plumbing around `GLOBAL` and tool management. The module
still uses `lock().unwrap()` and direct `PyRuntimeError`:
- `src/monitoring/tracer.rs:268`, `:270`, `:366`, `:432-706`
- `src/monitoring/mod.rs:123`

### Definition of Done
- Replace `unwrap` with fallible guard handling (`RecorderError` + policy).
- Install global panic-catching wrappers so monitoring callbacks never unwind
  into CPython.
- Add integration tests simulating poisoned mutexes and double-install calls.

### Status
Open


## ISSUE-014
### Description
Introduce structured Python exception hierarchy for user-facing APIs. The
module still raises built-ins:
- `codetracer_python_recorder/session.py:34`, `:57`, `:62`

### Definition of Done
- Define `RecorderError` Python base class and map subclasses to error kinds.
- Update API/unit tests to assert the new classes and associated error codes.
- Document upgrade guidance in the README/changelog.

### Status
Open
