# Module Call Event Naming – Implementation Plan

Plan owners: codetracer recorder maintainers  
Target ADR: 0014 – Module Call Events Report Actual Module Names  
Impacted components: `codetracer-python-recorder/src/runtime/tracer`, `src/runtime/value_capture`, `src/trace_filter`, design docs & user docs

## Goals
- Replace the `<module>` placeholder emitted during module imports with the actual dotted module name (e.g., `<boto3>`, `<foo.bar>`).
- Share a single, cached module identity resolver between trace filtering and runtime event recording so both systems agree on module names.
- Keep the hot path fast by caching module resolutions per code object and falling back to `<module>` only when we genuinely cannot determine the name.
- Provide regression tests proving the trace writer records the new labels for in-project modules, site-packages imports, and namespace packages with real filenames.

## Non-Goals
- Changing the trace file schema or emitting additional fields beyond the function name.
- Reworking how module names feed into configurable trace filters (that behaviour is covered by ADR 0013).
- Retrofitting the pure-Python recorder in this iteration, though the helper should make that future work straightforward.

## Current Gaps
- `RuntimeTracer::ensure_function_id` always uses `code.co_qualname`, which is `<module>` for every top-level module execution.
- Module name derivation logic lives inside `trace_filter::engine` and cannot be reused without duplicating heuristics (relative path stripping, `sys.modules` scan, `.pyc` awareness).
- There is no cache tying `code_id` → module name outside the filter cache, so even if we bolted on ad-hoc lookups we would keep rescanning `sys.modules`.
- No tests assert the textual function name for import events, so regressions would go unnoticed.

## Workstreams

### WS1 – Shared Module Identity Helper
**Scope:** Factor reusable module name derivation ahead of tracer integration.
- Extract `module_name_from_roots`, `lookup_module_name`, normalization helpers, and identifier validation from `trace_filter::engine` into a new module (e.g., `module_identity.rs`) under `src/common` or similar.
- Add APIs:
  - `ModuleIdentityResolver::from_sys_path(py) -> Self` to snapshot `sys.path` roots.
  - `fn resolve_for_code(&self, py, code, frame_globals_name: Option<&str>) -> Option<String>` that performs (1) cached lookup by code id, (2) relative path inference, (3) `sys.modules` scan, and (4) optional `__name__` override when provided.
- Allow the resolver to accept already-known module names (from `ScopeResolution.module_name`) so we do not recompute them.
- Write focused Rust unit tests that stub out `sys.modules` entries to cover `.py` vs `.pyc` matches, namespace packages (`package/__init__.py`), and failure cases.
- Update `trace_filter::engine` to depend on the new helper rather than its private copy, keeping behaviour identical.

### WS2 – Runtime Tracer Integration
**Scope:** Use the helper to rename `<module>` call events.
- Extend `RuntimeTracer` with a `module_name_cache: HashMap<usize, Option<String>>` (or wrap the helper inside the tracer) and ensure it is cleared when the tracer resets.
- When registering a call:
  - Check whether the filter resolution already cached a module name; pass it into the resolver to avoid recomputation.
  - Detect module-level code by checking `qualname == "<module>"` (and guard against variants like `"<module>"` with whitespace).
  - Ask the helper for a module name; when successful, format the function label as `format!("<{name}>")` before calling `TraceWriter::ensure_function_id`.
  - If the helper cannot decide, continue emitting `<module>` to preserve backwards compatibility.
- Consider inspecting the captured frame snapshot (already obtained for argument capture) to pull `globals["__name__"]` as an extra hint when `sys.modules` fails; plumb that optional string into the helper to keep the `ensure_function_id` signature narrow.
- Emit debug logs when we fall back to `<module>` despite having a real filename so troubleshooting remains possible.

### WS3 – Testing, Tooling, and Docs
**Scope:** Prove the behaviour change and explain it to users.
- Add Python integration tests (e.g., `tests/python/test_monitoring_events.py`) that:
  - Import a first-party module (`tests/python/support/sample_pkg/__init__.py`) and assert the trace JSON contains `<tests.python.support.sample_pkg>`.
  - Import a third-party-like package placed under a temporary directory inserted into `sys.path` to ensure `sys.modules` + relative path fallback works.
  - Cover namespace packages or packages that only expose `__spec__.name` to ensure we trust metadata over filesystem guesses.
- Add Rust tests for the resolver cache (verify we only touch `sys.modules` once per module) and for the formatting logic in `ensure_function_id`.
- Update `docs/README.md` (or recorder-specific docs) to mention that module names now appear in angle brackets, along with any troubleshooting guidance for cases where `<module>` persists (synthetic filenames, frozen imports).
- Refresh changelog entries and any CLI snapshot tests that checked for `<module>`.

## Testing Strategy
- `cargo test -p codetracer-python-recorder` to run the new resolver unit tests.
- `just test` to exercise Python integration tests and ensure traces serialize as expected.
- Manual verification script that traces `python - <<'PY' ...` importing `boto3` (or a stub) and inspects the `.trace` file with `runtime_tracing::TraceReader`.

## Risks & Mitigations
- **Performance regressions:** Taking the GIL and scanning `sys.modules` per module could add overhead. Mitigate via per-code caches and by reusing filter-derived names when available.
- **Incorrect attribution:** Namespace packages or reloaded modules might map multiple files to one name. Mitigate by preferring `__spec__.name`/`__name__` and logging whenever multiple candidates compete.
- **Tight coupling with filters:** If the helper accidentally references filter-specific types, it becomes impossible to reuse elsewhere. Keep the helper independent and inject filter results as plain `Option<String>`.
- **Test brittleness:** Snapshot tests referencing `<module>` need refreshing; add helper assertions that look for `<...>` pattern rather than hard-coded `<module>`.

## Open Questions
- Should we expose the resolved module name anywhere else (e.g., as metadata on trace records) to aid scripting? For now we only change the function label, but we might want to surface the raw dotted name later.
- How should we treat modules executed via `runpy.run_module` with `__name__ == "__main__"` but living under a package path? The helper will return `"__main__"`; confirm that is acceptable or consider using the package dotted path derived from filename when `__name__ == "__main__"`.
