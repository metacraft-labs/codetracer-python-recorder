# Module Name Resolution via `__name__` – Implementation Plan

This plan delivers ADR 0016 by retooling module-name derivation around `frame.f_globals["__name__"]` and retiring the existing filesystem-based resolver.

## Goals
- Use `__name__` as the single source of truth for module identifiers during tracing and filtering.
- Remove the `ModuleIdentityResolver`/`ModuleIdentityCache` complex and associated heuristics.
- Preserve relative/absolute path metadata for selectors and telemetry.
- Ship the change behind a feature flag so we can roll out gradually.

## Non-goals
- Changing how file selectors work (they should keep matching on normalised paths).
- Replacing hints elsewhere that already rely on `__name__` (e.g., value redaction logic).
- Revisiting logging or module instrumentation beyond the tracer/filter flow.

## Work Breakdown

### Stage 0 – Feature Flag and Compatibility Layer
- Add a recorder policy flag `module_name_from_globals` defaulting to `false`.
- Plumb the flag through CLI/env configuration and expose it via the Python bindings (mirroring other policy toggles).
- Update integration tests to parameterise expectations for both modes.

### Stage 1 – Capture `__name__` at `py_start`
- In `runtime/tracer/events.rs`, augment the `on_py_start` handler to detect `<module>` code objects.
- Fetch `frame.f_globals.get("__name__")`, validate it is a non-empty string, and store it in the scope resolution cache (likely inside `FilterCoordinator`).
- Thread the captured value into `FilterCoordinator::resolve` so the filter engine obtains the module name without invoking the resolver.
- Add unit tests that simulate modules with various `__name__` values (`__main__`, aliased imports, missing globals) to ensure fallbacks log appropriately.

### Stage 2 – Simplify Filter Engine
- Remove the `module_resolver` field from `TraceFilterEngine` and delete calls to `ModuleIdentityResolver::resolve_absolute` (`codetracer-python-recorder/src/trace_filter/engine.rs:183`).
- Adjust `ScopeContext` to accept the module name supplied by the coordinator and skip path-derived module inference.
- Keep relative and absolute path normalisation for file selectors.
- Update Rust tests that previously expected filesystem-derived names to assert the new globals-based behaviour.

### Stage 3 – Replace Runtime Function Naming
- Eliminate `ModuleIdentityCache` from `RuntimeTracer` and switch `function_name` to prefer the cached globals-derived module name (`runtime_tracer.rs:280`).
- Remove the `ModuleNameHints` plumbing and any resolver invocations.
- Update the Python integration test `test_module_imports_record_package_names` to expect `<my_pkg.mod>` coming directly from `__name__`.

### Stage 4 – Documentation and Changelog
- Update design docs, ADR status, and changelog entries to describe the new globals-based module naming.
- Ensure the developer guide explains how `__name__` hints interplay with filter selectors and `<__main__>` behaviour.
- Document the phase-out of the resolver to guide downstream integrators.

### Stage 5 – Flip the Feature Flag
- Change the default for `module_name_from_globals` to `true` while leaving CLI/env toggles available for targeted opt-outs during rollout validation.
- Schedule removal of the compatibility flag once usage data confirms no regressions.

## Testing Strategy
- Rust unit tests covering `on_py_start` logic, especially fallback to `<module>` when `__name__` is absent.
- Python integration tests for direct scripts (`__main__`), package imports, and dynamic `runpy` execution to capture expected names.
- Regression test ensuring filters still match file-based selectors when module names change.
- Performance check to confirm hot-path allocations decreased after removing resolver lookups.

## Risks & Mitigations
- **Filter regressions for direct scripts:** Document the `"__main__"` behaviour and add guardrails in tests; optionally add helper patterns in builtin filters.
- **Third-party loaders with non-string `__name__`:** Validate type and fall back gracefully when extraction fails.
- **Hidden dependencies on old naming:** Continue exposing absolute/relative paths in `ScopeResolution` so downstream tooling relying on paths keeps functioning.

## Rollout Checklist
- ADR 0016 updated to “Accepted” once the feature flag defaults to on.
- Release notes highlight the new module-naming semantics and the opt-out flag during transition.
- Telemetry or logging confirms we do not hit the fallback path excessively in real workloads.
