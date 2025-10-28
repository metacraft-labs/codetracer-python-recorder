# Reliable Module Name Derivation – Status

## Relevant Design Docs
- `design-docs/adr/0013-reliable-module-name-derivation.md`
- `design-docs/reliable-module-name-derivation-implementation-plan.md`

## Key Source Files
- `codetracer-python-recorder/src/trace_filter/engine.rs`
- `codetracer-python-recorder/src/trace_filter/scope.rs` *(ScopeContext helpers live here via engine module)*
- `codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs`
- `docs/onboarding/trace-filters.md`
- `codetracer-python-recorder/resources/trace_filters/builtin_default.toml`

## Workstream Progress
- ✅ **WS1 – Context Inspection & Cache Design:** Reviewed the `ScopeContext` derivation flow, confirmed why inline filters only saw `"."` project roots, and identified the need for a shared module-name cache plus sys.path-derived fallbacks.
- ✅ **WS2 – sys.modules/sys.path Fallback & Diagnostics:** Added a per-engine module cache, captured normalized `sys.path` roots at construction time, implemented a resolver that prefers path-derived module names but can fall back to `sys.modules`, and logged when neither strategy succeeds.
- ✅ **WS3 – Regression Tests & Docs:** Added the `inline_pkg_rule_uses_sys_modules_fallback` regression test to guard the behaviour, documented the new module-derivation path in `docs/onboarding/trace-filters.md`, and ensured `just dev test` (Rust + Python) passes end-to-end.

## Next Update
Future updates will only be necessary if follow-up work (e.g., exposing additional diagnostics or handling namespace packages) is scheduled.
