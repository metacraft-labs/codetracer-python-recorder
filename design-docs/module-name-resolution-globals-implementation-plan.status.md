# Module Name Resolution via `__name__` – Status

## Relevant Design Docs
- `design-docs/adr/0016-module-name-resolution-via-globals-name.md`
- `design-docs/module-name-resolution-globals-implementation-plan.md`

## Key Source Files
- `codetracer-python-recorder/src/policy/model.rs`
- `codetracer-python-recorder/src/policy/env.rs`
- `codetracer-python-recorder/src/policy/ffi.rs`
- `codetracer-python-recorder/src/policy.rs`
- `codetracer-python-recorder/codetracer_python_recorder/cli.py`
- `codetracer-python-recorder/codetracer_python_recorder/session.py`
- `codetracer-python-recorder/tests/python/test_policy_configuration.py`
- `codetracer-python-recorder/tests/python/unit/test_cli.py`

## Workstream Progress

### Stage 0 – Feature Flag and Compatibility Layer
- **Status:** Completed  
  Added the `module_name_from_globals` policy flag with CLI flag, Python bindings, and the `CODETRACER_MODULE_NAME_FROM_GLOBALS` env hook. Regression tests cover CLI parsing, policy snapshots, and environment configuration.

### Stage 1 – Capture `__name__` at `py_start`
- **Status:** Completed  
  `RuntimeTracer` now captures `frame.f_globals['__name__']` for `<module>` code when the feature flag is on, threads the hint through `FilterCoordinator`, and prefers it during both filter decisions and runtime naming. Added integration coverage ensuring opt-in sessions record `<__main__>` for scripts, plus unit updates for the new plumbing.

### Stage 2 – Simplify Filter Engine
- **Status:** Completed  
  Removed the resolver dependency from `TraceFilterEngine`, teach it to accept globals-based hints, and added a package-structure fallback when relative paths are unavailable. Filtering decisions now rely on the new hint flow while keeping legacy behaviour intact for `module_name_from_globals` opt-out scenarios.

### Stage 3 – Runtime Naming Downstream
- **Status:** Completed  
  `RuntimeTracer` no longer depends on `ModuleIdentityCache`; it prefers globals hints, otherwise reuses filter resolutions, `sys.path` roots, or package markers before falling back to `<module>`. The shared helpers in `module_identity.rs` were slimmed down accordingly, and updated unit/integration tests confirm the new flow.
