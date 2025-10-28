# Module Call Event Naming – Status

## Relevant Design Docs
- `design-docs/adr/0014-module-call-event-naming.md`
- `design-docs/module-call-event-naming-implementation-plan.md`
- `design-docs/adr/0013-reliable-module-name-derivation.md`

## Key Source Files
- `codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs`
- `codetracer-python-recorder/src/runtime/tracer/events.rs`
- `codetracer-python-recorder/src/runtime/value_capture.rs`
- `codetracer-python-recorder/src/runtime/frame_inspector.rs`
- `codetracer-python-recorder/src/trace_filter/engine.rs`
- `codetracer-python-recorder/tests/python/test_monitoring_events.py`
- `codetracer-python-recorder/tests/rust`

## Workstream Progress

### WS1 – Shared Module Identity Helper
- **Scope recap:** Extract and centralise module-name derivation (relative path stripping + `sys.modules` lookup) so both filters and the runtime tracer can reuse it with caching.
- **Status:** _Completed_
-- **Notes:** `src/module_identity.rs` now provides the lightweight helpers (`module_from_relative`, `module_name_from_packages`, etc.) that both the tracer and filter engine reuse when hints are missing, keeping naming consistent across components.

### WS2 – Runtime Tracer Integration
- **Scope recap:** Detect module-level code (`co_qualname == "<module>"`) and rename call events to `<module-name>` using the shared resolver; plumb filter-derived names to avoid duplicate work.
- **Status:** _Completed_
-- **Notes:** `RuntimeTracer` rewrites module-level call events using the globals-derived hint or the filter’s cached resolution, with integration tests (`module_import_records_module_name`, `test_module_imports_record_package_names`) confirming `<pkg.mod>` appears in `trace.json`.

### WS3 – Testing, Tooling, and Docs
- **Scope recap:** Add regression tests (Python + Rust) validating the new naming, update documentation/changelog, and refresh any snapshot expectations.
- **Status:** _Completed_
- **Notes:** Added a Rust unit test plus an integration test in `tests/python/test_monitoring_events.py`, documented the behaviour in the README, and recorded the change in `CHANGELOG.md`. Snapshot consumers now rely on the `<pkg.module>` naming convention.

## Next Checkpoints
1. Implement shared resolver scaffolding (WS1).
2. Wire runtime tracer and verify traces emit `<pkg>` names (WS2).
3. Land regression tests and docs, then update this status file accordingly (WS3).
