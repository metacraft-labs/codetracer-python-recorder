# ADR 0004: Error Handling Policy for codetracer-python-recorder

- **Status:** Proposed
- **Date:** 2025-10-02
- **Deciders:** Runtime Tracing Maintainers
- **Consulted:** Python Tooling WG, Observability WG
- **Informed:** Developer Experience WG, Release Engineering

## Context

The Rust-backed recorder currently propagates errors piecemeal:
- PyO3 entry points bubble up plain `PyRuntimeError` instances with free-form strings (e.g., `src/session.rs:21-52`, `src/runtime/mod.rs:77-126`).
- Runtime helpers panic on invariant violations, which will abort the host interpreter because we do not fence panics at the FFI boundary (`src/runtime/mod.rs:107-120`, `src/runtime/activation.rs:24-33`, `src/runtime/value_encoder.rs:61-78`).
- Monitoring callbacks rely on `GLOBAL.lock().unwrap()` so poisoned mutexes or lock errors terminate the process (`src/monitoring/tracer.rs:268` and subsequent callback shims).
- Python helpers expose bare `RuntimeError`/`ValueError` without linking to a shared policy, and auto-start simply re-raises whatever the Rust layer emits (`codetracer_python_recorder/session.py:27-63`, `codetracer_python_recorder/auto_start.py:24-36`).
- Exit codes, log destinations, and trace-writer fallback behaviour are implicit; a disk-full failure today yields a generic exception and can leave partially written outputs.

The lack of a central error façade makes it hard to enforce user-facing guarantees, reason about detaching vs aborting behaviour, or meet the operational goals we have been given: stable error codes, structured logs, optional JSON diagnostics, policy switches, and atomic trace outputs.

## Decision

We will introduce a recorder-wide error handling policy centred on a dedicated `recorder-errors` crate and a Python exception hierarchy. The policy follows fifteen guiding principles supplied by operations and is designed so the “right way” is the only easy way for contributors.

### 1. Single Error Façade
- Create a new workspace crate `recorder-errors` exporting `RecorderError`, a structural error type with fields `{ kind: ErrorKind, code: ErrorCode, message: Cow<'static, str>, context: ContextMap, source: RecorderErrorSource }`.
- Provide `RecorderResult<T> = Result<T, RecorderError>` and convenience macros (`usage!`, `enverr!`, `target!`, `bug!`, `ensure_usage!`, `ensure_env!`, etc.) so Rust modules can author classified failures with one line.
- Require every other crate (including the PyO3 module) to depend on `recorder-errors`; direct construction of `PyErr`/`io::Error` is disallowed outside the façade.
- Maintain `ErrorCode` as a small, grep-able enum (e.g., `ERR_TRACE_DIR_NOT_DIR`, `ERR_FORMAT_UNSUPPORTED`), with documentation in the crate so codes stay stable across releases.

### 2. Clear Classification & Exit Codes
- Define four top-level `ErrorKind` variants:
  - `Usage` (caller mistakes, bad flags, conflicting sessions).
  - `Environment` (IO, permissions, resource exhaustion).
  - `Target` (user code raised or misbehaved while being traced).
  - `Internal` (bugs, invariants, unexpected panics).
- Map kinds to fixed process exit codes (`Usage=2`, `Environment=10`, `Target=20`, `Internal=70`). These are surfaced by CLI utilities and exported via the Python module for embedding tooling.
- Document canonical examples for each kind in the ADR appendix and in crate docs.

### 3. FFI Safety & Python Exceptions
- Add an `ffi` module that wraps every `#[pyfunction]` with `catch_unwind`, converts `RecorderError` into a custom Python exception hierarchy (`RecorderError` base, subclasses `UsageError`, `EnvironmentError`, `TargetError`, `InternalError`), and logs panic payloads before mapping them to `InternalError`.
- PyO3 callbacks (`install_tracer`, monitoring trampolines) will run through `ffi::dispatch`, ensuring we never leak panics across the boundary.

### 4. Output Channels & Diagnostics
- Forbid `println!`/`eprintln!` outside the logging module; diagnostic output goes to stderr via `tracing`/`log` infrastructure.
- Introduce a structured logging wrapper that attaches `{ run_id, trace_id, error_code }` fields to every error record. Provide `--log-level`, `--log-file`, and `--json-errors` switches that route structured diagnostics either to stderr or a configured file.

### 5. Policy Switches
- Introduce a runtime policy singleton (`RecorderPolicy` stored in `OnceCell`) configured via CLI flags or environment variables: `--on-recorder-error=abort|disable`, `--require-trace`, `--keep-partial-trace`.
- Define semantics: `abort` -> propagate error and non-zero exit; `disable` -> detach tracer, emit structured warning, continue host process. Document exit codes for each combination in module docs.

### 6. Atomic, Truthful Outputs
- Wrap trace writes behind an IO façade that stages files in a temp directory and performs atomic rename on success.
- When `--keep-partial-trace` is enabled, mark outputs with a `partial=true`, `reason=<ErrorCode>` trailer. Otherwise ensure no trace files are left behind on failure.

### 7. Assertions with Containment
- Replace `expect`/`unwrap` (e.g., `src/runtime/mod.rs:114`, `src/runtime/activation.rs:26`, `src/runtime/value_encoder.rs:70`) with classified `bug!` assertions that convert to `RecorderError` while still triggering `debug_assert!` in dev builds.
- Document invariants in the new crate and ensure fuzzing/tests observe the diagnostics.

### 8. Preflight Checks
- Centralise version/compatibility checks in a `preflight` module called from `start_tracing`. Validate Python major.minor, ABI compatibility, trace schema version, and feature flags before installing monitoring callbacks.
- Embed recorder version, schema version, and policy hash into every trace metadata file via `TraceWriter` extensions.

### 9. Observability & Metrics
- Emit structured counters for key error pathways (dropped events, detach reasons, panics caught). Provide a `RecorderMetrics` sink with a no-op default and an optional exporter trait.
- When `--json-errors` is set, append a single-line JSON trailer to stderr containing `{ "error_code": .., "kind": .., "message": .., "context": .. }`.

### 10. Failure-Path Testing
- Add exhaustive unit tests in `recorder-errors` for every `ErrorCode` and conversion path.
- Extend Rust integration tests to simulate disk-full (`ENOSPC`), permission denied, target exceptions, callback panics, SIGINT during detach, and partial trace recovery.
- Add Python tests asserting the custom exception hierarchy and policy toggles behave as documented.

### 11. Performance-Aware Defences
- Reserve heavyweight diagnostics (stack captures, large context maps) for error paths. Hot callbacks use cheap checks (`debug_assert!` in release builds). Provide sampled validation hooks if additional runtime checks become necessary.

### 12. Tooling Enforcement
- Add workspace lints (`deny(panic_in_result_fn)`, Clippy config) and a `just lint-errors` task that fails if `panic!`, `unwrap`, or `expect` appear outside `recorder-errors`.
- Disallow `anyhow`/`eyre` except inside the error façade with documented justification.

### 13. Developer Ergonomics
- Export prelude modules (`use recorder_errors::prelude::*;`) so contributors get macros and types with a single import.
- Provide cookbook examples in the crate documentation and link the ADR so developers know how to map new errors to codes quickly.

### 14. Documented Guarantees
- Document, in README + crate docs, the three promises: no stdout writes, trace outputs are atomic (or explicitly partial), and error codes stay stable within a minor version line.

### 15. Scope & Non-Goals
- The recorder never aborts the host process; even internal bugs downgrade to `InternalError` surfaced through policy switches.
- Business-specific retention, shipping logs, or analytics integrations remain out of scope for this ADR.

## Consequences

- **Positive:** Structured errors enable user tooling, stable exit codes improve scripting, and panics are contained so we remain embedder-friendly. Central macros reduce boilerplate and make reviewers enforce policy easily.
- **Negative / Risks:** Introducing a new crate and policy layer adds upfront work and requires retrofitting existing call sites. Atomic IO staging may increase disk usage for large traces. Contributors must learn the new taxonomy and update tests accordingly.

## Rollout & Status Tracking

- Implementation proceeds under a dedicated plan (see "Error Handling Implementation Plan"). The ADR moves to **Accepted** once the façade crate, FFI wrappers, and policy switches are merged, and the legacy ad-hoc errors are removed.
- Future adjustments (e.g., new error codes) must update `recorder-errors` documentation and ensure backward compatibility for exit codes.

## Alternatives Considered

- **Use `anyhow` throughout and convert at the boundary.** Rejected because it obscures error provenance, offers no stable codes, and encourages stringly-typed errors.
- **Catch panics lazily within individual callbacks.** Rejected; a central wrapper keeps the policy uniform and ensures we do not miss newer entry points.
- **Rely on existing logging without policy switches.** Rejected because operational requirements demand scriptable behaviour on failure.

