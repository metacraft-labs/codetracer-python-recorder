# ADR 0017 – Recorder Exit Code Policy

- **Status:** Proposed
- **Date:** 2025-10-28
- **Stakeholders:** Desktop CLI team, Runtime tracer maintainers, Release engineering
- **Related Decisions:** ADR 0005 (Python Recorder DB Backend Integration), ADR 0015 (Balanced Toplevel Lifecycle and Trace Gating)

## Context

`ct record` invokes the Rust-backed `codetracer_python_recorder` CLI when capturing Python traces. The CLI currently returns the traced script's process exit code (`codetracer_python_recorder/cli.py:165`). When the target program exits with a non-zero status—whether via `SystemExit`, a failed assertion, or an explicit `sys.exit()`—the recorder propagates that status. The desktop CLI treats any non-zero exit as a fatal recording failure, so trace uploads and follow-on automation abort even though the trace artefacts are valid and the recorder itself completed successfully.

Our recorder already captures the script's exit status in session metadata (`runtime/tracer/lifecycle.rs:143`) and exposes it through trace viewers. Downstream consumers that need to assert on the original program outcome can read that field. However, other integrations (CI pipelines, `ct record` automations, scripted data collection) rely on the CLI process exit code to decide whether to continue, and they expect Codetracer to return `0` when recording succeeded.

We must let callers control whether the recorder propagates the script's exit status or reports recorder success independently. The default should favour Codetracer success (exit `0`) to preserve `ct record` expectations, while still allowing advanced users and direct CLI invocations to opt back into passthrough semantics.

## Decision

Introduce a recorder exit-code policy with the following behaviour:

1. **Default:** When tracing completes without recorder errors (start, flush, stop, and write phases succeed and `require_trace` did not trigger), the CLI exits with status `0` regardless of the traced script's exit code. The recorder still records the script's status in trace metadata.
2. **Opt-in passthrough:** Expose a CLI flag `--propagate-script-exit` and environment override `CODETRACER_PROPAGATE_SCRIPT_EXIT`. When enabled, the CLI mirrors the traced script's exit code (the current behaviour). Both configuration surfaces resolve through the recorder policy layer so other entry points (e.g., embedded integrations) can opt in.
3. **User feedback:** If passthrough is disabled and the script exits non-zero, emit a one-line warning on stderr indicating the script's exit status and how to re-enable propagation.
4. **Recorder failure precedence:** Recorder failures (startup errors, policy violations such as `--require-trace`, flush/stop exceptions) continue to exit non-zero irrespective of the propagation setting to ensure automation can detect recorder malfunction.

This policy applies uniformly to `python -m codetracer_python_recorder`, `ct record`, and any embedding that drives the same CLI module.

## Consequences

**Positive**

- `ct record` can treat successful recordings as success even when the target script fails, unblocking chained workflows and uploads.
- The script's exit status remains available in trace metadata, preserving observability without overloading process exit handling.
- Configuration is explicit and discoverable via CLI help, environment variables, and policy APIs.

**Negative / Risks**

- Direct CLI users may miss that their script failed if they rely solely on the process exit code. The stderr warning mitigates this but adds additional output.
- Changing the default may surprise users accustomed to passthrough semantics. Documentation and release notes must highlight the new default and the opt-in flag.
- Additional configuration surface increases policy complexity; we must ensure conflicting overrides (CLI vs. env) resolve predictably.

## Rollout Notes

- Update CLI help text, README, and desktop `ct record` documentation to describe the new default and override flag.
- Add regression tests covering both default and passthrough modes (CLI invocation, environment override, policy API).
- Communicate the change in the recorder CHANGELOG and release notes so downstream automation owners can adjust expectations.
