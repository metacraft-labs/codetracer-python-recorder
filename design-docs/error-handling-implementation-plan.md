# Error Handling Plan (Fast Read)

## Aim
Give the recorder one predictable error story: every failure becomes a `RecorderError`, maps to a stable code, and picks the right Python exception.

## Current pain
- Random `PyRuntimeError` strings leak out of `session`, `runtime`, and `monitoring` code.
- `unwrap` / `expect` still show up in FFI trampolines, so real errors can crash Python.
- The Python wrapper raises built-in exceptions with no extra context.
- Partial trace files appear when I/O dies mid-run.

## Work plan
1. **Audit**
   - Add a `just errors-audit` helper that lists every `PyRuntimeError`, `unwrap`, `expect`, and `panic!` in the recorder crate.
   - File follow-up issues assigning owners to the hotspots.
2. **`recorder-errors` crate**
   - Create a small crate with `RecorderError`, `RecorderResult`, `ErrorKind`, and `ErrorCode` enums.
   - Provide conversions from `io::Error`, `PyErr`, and internal helper types.
   - Offer macros like `bail_recorder!(kind, code, "message")` so callers stay concise.
3. **Rust call sites**
   - Replace ad-hoc error strings with the new enums.
   - Swap `unwrap`/`expect` for `?` or explicit matches.
   - Ensure long-running loops decide between “abort the process” and “disable tracing” using one policy helper.
4. **Python facade**
   - Map each `RecorderError` to a concrete Python exception with a deterministic message and optional structured payload.
   - Surface diagnostics (JSON preferred) that tools can consume.
5. **Atomic output**
   - Stage traces under a temp directory, then rename when complete.
   - If something fails, mark the trace as partial and clean up temp files.
6. **Testing**
   - Add unit tests for conversion helpers and policy branches.
   - Write integration tests that simulate disk failures and poisoning scenarios to prove we no longer panic.

## Definition of done
- No `unwrap` / `expect` in the FFI boundary or runtime hot path.
- All exported Python functions raise our mapped exceptions.
- Temp files clean up correctly during forced failures.
- Test suite covers success, handled failure, and aborting failure paths.
