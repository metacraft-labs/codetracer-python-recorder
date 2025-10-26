# Reliable Module Name Derivation – Implementation Plan

## Summary
Trace filter package selectors currently fail for inline (builtin) filters because the engine can only derive module names by stripping filter `project_root` prefixes from filenames. This plan delivers the ADR 0013 decision: introduce a `sys.modules`-based fallback with caching so builtin and inline filters can match `pkg:*` selectors reliably, while keeping existing relative-path logic fast for project-local code.

## Goals
1. Package selectors from any filter source (inline or on-disk) resolve consistently for real files (`_distutils_hack`, setuptools helpers, etc.).
2. The fallback module lookup incurs minimal overhead by caching filename→module mappings.
3. Diagnostics exist for frames where module derivation still fails (synthetic filenames, missing `__file__`), so users can understand selector misses.
4. Regression coverage demonstrates the builtin `_distutils_hack` skip works end-to-end.

## Non-goals
- Changing filter syntax or adding new selector kinds.
- Supporting synthetic filenames (`<string>`, `<stdin>`) beyond today’s behaviour.
- Tracking module reloads hot; we assume module identities stay stable within one tracing session.

## Work Breakdown

### 1. Engine Data Model Updates
- Extend `ScopeContext` to carry a reference-counted cache (e.g., `DashMap<PathBuf, Option<String>>`) mapping canonical filenames to module names.
- Thread this cache through `TraceFilterEngine` so all resolutions share it.
- Keep the existing `project_root` stripping logic as the first attempt.

### 2. `sys.modules` Fallback
- Implement a helper `resolve_module_via_sys_modules(py, filename) -> Option<String>`:
  - Canonicalise `filename`.
  - Iterate over `sys.modules.items()` (skipping `None` entries), inspect `__file__` attributes, and compare after normalisation.
  - Return the module name (dictionary key) when the path matches.
  - Cache successes and failures.
- Ensure we hold the GIL while accessing Python objects and release references promptly.

### 3. Diagnostics
- When both prefix stripping and `sys.modules` lookup fail, emit a `log::debug!` with the filename so advanced users can trace why selectors do not match.
- Optionally increment an internal metric counter for “module derivation fallback failures” to aid telemetry.

### 4. Builtin Filter Verification
- Add/extend a runtime test that traces a trivial script importing `_distutils_hack` (simulate by touching site-packages path) and assert the builtin filter now skips those frames.
- Update existing tests to cover the fallback path (e.g., by constructing a filter source with `project_root="."`).

### 5. Documentation
- Update `docs/onboarding/trace-filters.md` to explain how module names are derived and that builtin filters now correctly skip site-packages helpers.
- Reference ADR 0013 from the configurable trace filters design doc for posterity.

## Testing Strategy
- Unit tests for the new lookup helper, using temporary modules inserted into `sys.modules` with fake `__file__` values.
- Integration test exercising `pkg:literal:_distutils_hack` via the builtin filter chain.
- Regression test ensuring the cache handles multiple resolutions of the same file without repeated scans (can assert the helper is only called once via instrumentation).

## Risks & Mitigations
- **Performance:** Scanning `sys.modules` could be expensive. Mitigate with canonical-path caching and only invoking the fallback when path stripping fails.
- **Thread safety:** Accessing Python objects without the GIL would be unsafe. Keep all fallback work within the `Python<'_>` context already available in `TraceFilterEngine::resolve`.
- **Stale cache entries:** Module files rarely change mid-run; if they do, the trace filter outcome would still be correct because module names remain stable. Document this assumption.

## Timeline (rough)
1. Day 0–1: Land ADR + plan (this document).
2. Day 2–3: Implement cache plumbing and fallback helper with unit tests.
3. Day 4: Wire into `ScopeContext::derive`, add diagnostics.
4. Day 5: Expand runtime/integration tests, update docs.
5. Day 6: Code review, land, monitor CI/perf.

## Open Questions
- Should we normalise module names (e.g., remove `.pyc` suffixes) when reading from `sys.modules`? (default answer: yes, strip `.pyc` to match `.py` filenames.)
- Do we need to support namespace packages with multiple `__file__` entries? (likely postpone; first implementation can stop at the first matching entry.)
