# codetracer-python-recorder Error Handling Implementation Plan

## Goals
- Deliver the policy defined in ADR 0004: every error flows through `RecorderError`, surfaces a stable code/kind, and maps to the Python exception hierarchy.
- Contain all panics within the FFI boundary and offer deterministic behaviour for `abort` versus `disable` policies.
- Ensure trace outputs remain atomic (or explicitly marked partial) and diagnostics never leak to stdout.
- Provide developers with ergonomic macros, tooling guardrails, and comprehensive tests covering failure paths.

## Current Gaps
- Ad-hoc `PyRuntimeError` strings in `src/session.rs:21-76` and `src/runtime/mod.rs:77-190` prevent stable categorisation and user scripting.
- FFI trampolines in `src/monitoring/tracer.rs:268-706` and activation helpers in `src/runtime/activation.rs:24-83` still use `unwrap`/`expect`, so poisoned locks or filesystem errors abort the interpreter.
- Python facade functions (`codetracer_python_recorder/session.py:27-63`) return built-in exceptions and provide no context or exit codes.
- No support for JSON diagnostics, policy switches, or atomic output staging; disk failures can leave half-written traces and logs mix stdout/stderr.

## Workstreams

### WS1 – Foundations & Inventory
**Status:** In progress (2025-10-02). `just errors-audit` added; call sites catalogued in the accompanying status log.
- Add a `just errors-audit` command that runs `rg` to list `PyRuntimeError`, `unwrap`, `expect`, and direct `panic!` usage in the recorder crate.
- Create issue tracker entries grouping call sites by module (`session`, `runtime`, `monitoring`, Python facade) to guide refactors.
- Exit criteria: checklist of legacy error sites recorded with owners.

### WS2 – `recorder-errors` Crate
**Status:** Completed (2025-10-02). Crate scaffolded with central types, macros, and unit tests; workspace updated to include it.
- Scaffold `recorder-errors` under the workspace with `RecorderError`, `RecorderResult`, `ErrorKind`, `ErrorCode`, context map type, and conversion traits from `io::Error`, `PyErr`, etc.
- Implement ergonomic macros (`usage!`, `enverr!`, `target!`, `bug!`, `ensure_*`) plus unit tests covering formatting, context propagation, and downcasting.
- Publish crate docs explaining mapping rules and promises; link ADR 0004.
- Exit criteria: `cargo test -p recorder-errors` covers all codes; workspace builds with the new crate.

### WS3 – Retrofit Rust Modules
- Replace direct `PyRuntimeError` construction in `src/session/bootstrap.rs`, `src/session.rs`, `src/runtime/mod.rs`, `src/runtime/output_paths.rs`, and helpers with `RecorderResult` + macros.
- Update `RuntimeTracer` to propagate structured errors instead of strings; remove `expect`/`unwrap` in hot paths by returning classified `bug!` or `enverr!` failures.
- Introduce a small adapter in `src/runtime/mod.rs` that stages IO writes and applies the atomic/partial policy described in ADR 0004.
- Exit criteria: All recorder crate modules compile without `pyo3::exceptions::PyRuntimeError::new_err` usage.

### WS4 – FFI Wrapper & Python Exception Hierarchy
- Implement `ffi::wrap_pyfunction` that catches panics (`std::panic::catch_unwind`), maps `RecorderError` to a new `PyRecorderError` base type plus subclasses (`PyUsageError`, `PyEnvironmentError`, etc.).
- Update `#[pymodule]` and every `#[pyfunction]` to use the wrapper; ensure monitoring callbacks also go through the dispatcher.
- Expose the exception types in `codetracer_python_recorder/__init__.py` for Python callers.
- Exit criteria: Rust panics surface as `PyInternalError`, and Python tests can assert exception class + code.

### WS5 – Policy Switches & Runtime Configuration
- Add `RecorderPolicy` backed by `OnceCell` with setters for CLI flags/env vars: `--on-recorder-error`, `--require-trace`, `--keep-partial-trace`, `--log-level`, `--log-file`, `--json-errors`.
- Update the CLI/embedding entry points (auto-start, `TraceSession`) to fill the policy before starting tracing.
- Implement detach vs abort semantics in `RuntimeTracer::finish` / session stop paths, honoring policy decisions and exit codes.
- Exit criteria: Integration tests demonstrate both `abort` and `disable` flows, including partial trace handling.

### WS6 – Logging, Metrics, and Diagnostics
- Replace `env_logger` initialisation with a `tracing` subscriber or structured `log` formatter that includes `run_id`, `trace_id`, and `ErrorCode` fields.
- Emit counters for dropped events, detach reasons, and caught panics via a `RecorderMetrics` sink (default no-op, pluggable in future).
- Implement `--json-errors` to emit a single-line JSON trailer on stderr whenever an error is returned to Python.
- Exit criteria: Structured log output verified in tests; stdout usage gated by lint.

### WS7 – Test Coverage & Tooling Enforcement
- Add unit tests for the new error crate, IO façade, policy switches, and FFI wrappers (panic capture, exception mapping).
- Extend Python tests to cover the new exception hierarchy, JSON diagnostics, and policy flags.
- Introduce CI lints (`cargo clippy --deny clippy::panic`, custom script rejecting `unwrap` outside allowed modules) and integrate with `just lint`.
- Exit criteria: CI blocks regressions; failure-path tests cover disk full, permission denied, target exceptions, partial trace recovery, and SIGINT during detach.

### WS8 – Documentation & Rollout
- Update README, API docs, and onboarding material to describe guarantees, exit codes, example snippets, and migration guidance for downstream tools.
- Add a change log entry summarising the policy and how to consume structured errors from Python.
- Track adoption status in `design-docs/error-handling-implementation-plan.status.md` (mirror existing planning artifacts).
- Exit criteria: Documentation merged, status file created, ADR 0004 promoted to **Accepted** once WS2–WS7 land.

## Milestones & Sequencing
1. **Milestone A – Foundations:** Complete WS1 and WS2 (error crate scaffold) in parallel; unblock later work.
2. **Milestone B – Core Refactor:** Deliver WS3 and WS4 together so Rust modules emit structured errors and Python sees the new exceptions.
3. **Milestone C – Policy & IO Guarantees:** Finish WS5 and WS6 to stabilise runtime behaviour and diagnostics.
4. **Milestone D – Hardening:** Execute WS7 (tests, tooling) and WS8 (documentation). Promote ADR 0004 to Accepted.

## Verification Strategy
- Add a `just test-errors` recipe running targeted failure tests (disk-full, detach, panic capture) plus Python unit tests for error classes.
- Use `cargo nextest run -p codetracer-python-recorder --features failure-fixtures` to execute synthetic failure cases.
- Enable `pytest tests/python/error_handling -q` for Python-specific coverage.
- Capture structured stderr in integration tests to assert JSON trailers and exit codes.

## Dependencies & Coordination
- Requires consensus with the Observability WG on log format fields and exit-code mapping.
- Policy flag wiring depends on any CLI/front-end work planned for Q4; coordinate with developer experience owners.
- If `runtime_tracing` needs extensions for metadata trailers, align timelines with that team.

## Risks & Mitigations
- **Wide-scope refactor:** Stage work behind feature branches and land per-module PRs to avoid blocking releases.
- **Performance regressions:** Benchmark hot callbacks before/after WS3 using existing microbenchmarks; keep additional allocations off hot paths.
- **API churn for users:** Provide compatibility shims that map old exceptions to new ones for at least one minor release, and document upgrade notes.
- **Partial trace semantics confusion:** Default to `abort` (no partial outputs) unless `--keep-partial-trace` is explicit; emit warnings when users opt in.

## Done Definition
- Legacy `PyRuntimeError::new_err` usage is removed or isolated to compat shims.
- All panics are caught before crossing into Python; fuzz tests confirm no UB.
- `just test` (and targeted error suites) pass on Linux/macOS CI, with new structured logs and metrics visible.
- Documentation reflects guarantees, and downstream teams acknowledge new exit codes.
