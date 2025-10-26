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
- **Status:** _In Progress_
- **Notes:** Created `src/module_identity.rs` housing `ModuleIdentityResolver`, a per-code `ModuleIdentityCache`, sanitisation helpers, and unit tests (covering `.py`/`.pyc`, package roots, and hint precedence). `TraceFilterEngine` now consumes the resolver instead of bespoke logic, and shared helpers (`module_from_relative`, `normalise_to_posix`, etc.) moved into the new module. `just dev` + `just test` succeed after the changeset; the cache will be hooked into `RuntimeTracer` during WS2.

### WS2 – Runtime Tracer Integration
- **Scope recap:** Detect module-level code (`co_qualname == "<module>"`) and rename call events to `<module-name>` using the shared resolver; plumb filter-derived names to avoid duplicate work.
- **Status:** _Not Started_
- **Notes:** Will require updates to `RuntimeTracer::ensure_function_id`, cache management, and potentially `capture_call_arguments` to surface `globals["__name__"]` when needed.

### WS3 – Testing, Tooling, and Docs
- **Scope recap:** Add regression tests (Python + Rust) validating the new naming, update documentation/changelog, and refresh any snapshot expectations.
- **Status:** _Not Started_
- **Notes:** Tests will likely live in `tests/python/test_monitoring_events.py` and a dedicated Rust module; docs update TBD.

## Next Checkpoints
1. Implement shared resolver scaffolding (WS1).
2. Wire runtime tracer and verify traces emit `<pkg>` names (WS2).
3. Land regression tests and docs, then update this status file accordingly (WS3).
