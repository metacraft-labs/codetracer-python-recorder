# Recorder Error Handling & Logging — User Stories

## Context
The current branch introduces the structured error-handling stack and logging upgrades outlined in the error-handling implementation plan. The stories below package those capabilities for product review.

## User Stories

### 1. Python clients receive structured recorder failures
**As** a Python integrator embedding the recorder
**I want** every failure to surface as a `RecorderError` (or subclass) with stable `ERR_*` codes and context metadata
**So that** my tooling can branch on error kind without parsing ad-hoc strings

**Acceptance criteria**
- Exceptions raised by the Rust extension map to `RecorderError`, `UsageError`, `EnvironmentError`, `TargetError`, or `InternalError`, each exposing `code`, `kind`, and `context` attributes populated from `RecorderError` fields.
- Panic conditions across the FFI boundary are caught and reclassified as `InternalError` with a distinct error code.

### 2. Embedders configure recorder policy centrally
**As** an application embedding CodeTracer
**I want** to configure runtime policy (abort vs disable, require traces, partial-trace retention, JSON errors) via Python APIs or environment variables before starting a session
**So that** the recorder’s shutdown behaviour matches my product’s UX and operational constraints

**Acceptance criteria**
- `configure_policy` accepts keyword arguments (e.g., `on_recorder_error`, `require_trace`, `keep_partial_trace`, `log_level`, `log_file`, `json_errors`) and updates the global policy snapshot consumed by the Rust runtime.
- `configure_policy_from_env` reads the `CODETRACER_*` environment variables and applies the same policy wiring automatically when packages import the module or the CLI starts.
- `TraceSession.start()` (and the CLI wrapper) refreshes policy from env, then forwards explicit overrides before activating tracing.

### 3. CLI operators can steer policy from the command line
**As** a CLI user running `python -m codetracer_python_recorder`
**I want** recorder policy toggles exposed as command-line flags alongside trace path/format options
**So that** I can experiment with abort vs disable flows, JSON trailers, and log destinations without writing glue code

**Acceptance criteria**
- The CLI accepts `--codetracer-on-recorder-error`, `--codetracer-require-trace`, `--codetracer-keep-partial-trace`, `--codetracer-log-level`, `--codetracer-log-file`, and `--codetracer-json-errors`, wiring them through `configure_policy`.
- When no explicit flags are provided, the CLI still honours policy derived from environment variables via `configure_policy_from_env`.

### 4. Structured diagnostics feed observability pipelines
**As** an observability engineer consuming recorder telemetry
**I want** recorder logs to emit structured JSON that includes a stable `run_id`, optional `trace_id`, log level, and any active error code
**So that** downstream collectors can correlate recorder failures with host application behaviour

**Acceptance criteria**
- Importing or starting tracing initialises the Rust logger once, generating JSON log lines with `run_id`, optional `trace_id`, `message`, and `error_code` fields.
- `with_error_code` scoping ensures error logs include the originating `ERR_*` value, and `set_active_trace_id` updates subsequent log entries during a trace.
- When `RecorderPolicy.log_file` is set, log output is redirected to the configured file; otherwise entries fall back to stderr with best-effort recovery on IO failures.

### 5. Automation can parse machine-readable error trailers
**As** a workflow owner triaging recorder failures
**I want** an opt-in JSON trailer on stderr describing each surfaced `RecorderError`
**So that** automated tooling can react to failure codes without scraping human text

**Acceptance criteria**
- Enabling the `json_errors` policy flag causes the FFI mapper to emit a JSON line with `run_id`, `trace_id`, `error_code`, `error_kind`, `message`, and `context` whenever a `RecorderError` crosses into Python.
- Trailer emission respects the configured writer (stderr by default, test hook override for automated verification) and flushes after each payload.

### 6. Metrics capture detachments and dropped events
**As** the recorder team monitoring runtime health
**I want** lightweight counters whenever tracing detaches, events are dropped, or panics are caught
**So that** we can detect regressions in sampling coverage and panic containment

**Acceptance criteria**
- A pluggable `RecorderMetrics` sink tracks `record_dropped_event`, `record_detach`, and `record_panic` calls, defaulting to a no-op until hosts install a collector.
- Runtime code invokes the metrics hooks when synthetic filenames are skipped, when policy-triggered detachments occur, and when the FFI wrapper captures a panic.

