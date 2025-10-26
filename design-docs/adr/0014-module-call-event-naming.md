# ADR 0014: Module Call Events Report Actual Module Names

- **Status:** Proposed
- **Date:** 2025-10-26
- **Deciders:** codetracer recorder maintainers
- **Consulted:** DX/observability stakeholders, Python runtime SMEs
- **Informed:** Support engineers, product analytics consumers

## Context
- Every Python import executes the target module object under the hood. The recorder hooks `PY_START` for those executions and emits a synthetic “function call” whose name currently comes from `code.co_qualname`.
- For module-level code objects CPython hardcodes `co_name == co_qualname == "<module>"`, so our trace file shows dozens (or hundreds) of `<module>` activations with no indication of which package was imported.
- Trace consumers (CLI visualisations, query tools, and data-engineering pipelines) rely on the recorded function name to surface hot paths, build flame graphs, and attribute time/cost to particular packages. Without module names, users cannot tell whether a slow import belongs to `boto3`, `_distutils_hack`, or their own modules.
- The trace filter engine already resolves module names from filenames / `sys.modules` to power `pkg:*` selectors, but the runtime tracer never reuses that information when assigning `FunctionId`s.

## Problem
- Module import events are indistinguishable in the trace log, making it impossible to attribute import costs, filter specific packages after the fact, or answer “which module executed here?” without cross-referencing filenames manually.
- Because traces only show `<module>`, downstream tools collapse all module-level activations into a single node, hiding per-package behaviour and producing misleading metrics.
- Purely using filenames as a proxy would leak physical layouts (e.g., `/usr/lib/python3.12/site-packages/boto3/__init__.py`) into the user-facing name and fails for zip apps or namespace packages.

## Decision
1. **Introduce a shared module identity helper.**
   - Extract the existing module-derivation logic (`module_name_from_roots`, `lookup_module_name`) into a reusable service that can run independent of the filter engine.
   - Accept a `CodeObjectWrapper` + `Python<'_>` handle and return a cached module name keyed by (code id, canonical filename). The helper first consults the filter resolution (if available), then repeats the relative-path inference, and finally falls back to `sys.modules` + frame globals (`__name__`) before conceding.
2. **Rewrite `<module>` names as `<{resolved_name}>`.**
   - When `RuntimeTracer::ensure_function_id` observes `co_qualname == "<module>"`, it queries the helper for the semantic module name.
   - If a valid dotted identifier comes back (e.g., `boto3`, `foo.bar`), the tracer registers the function as `<boto3>` / `<foo.bar>` so downstream tooling still recognises the syntactic angle-bracket convention while learning the package involved.
   - If resolution fails or yields garbage (non-identifier characters), the tracer keeps the legacy `<module>` label to avoid fabricating bad data.
3. **Cache aggressively and keep the hot path cheap.**
   - Store successful and failed lookups in `HashMap<usize, Option<String>>` keyed by code id so we only touch `sys.modules` or frame globals once per module.
   - Reuse the helper from both the runtime tracer and (later) the pure-Python recorder to ensure consistent naming semantics across products.
4. **Document the behaviour.**
   - Update the recorder docs to state that module-level call events now show `<module-name>` and call out the limited cases (synthetic filenames, frozen modules, namespace packages) where the fallback remains `<module>`.

## Consequences
- **Pros**
  - Import-heavy traces become readable: users immediately know which modules executed without digging through file paths.
  - Post-processing / analytics pipelines gain a stable key (the dotted module name) to aggregate import costs or identify slow third-party packages.
  - Reusing the existing derivation logic keeps behaviour aligned with trace filters and avoids duplicating heuristics.
- **Cons**
  - The tracer must occasionally scan `sys.modules` or inspect frame globals, which would add a small amount of work the first time we see each module. Caching mitigates the steady-state cost.
  - Changing the function label alters the textual output of existing traces, so snapshot tests or downstream expectations that explicitly compare against `<module>` need updates.
- **Risks**
  - Mis-resolving a module (e.g., due to namespace packages exposing multiple files) could misattribute work to the wrong name. We guard this by preferring explicit `__spec__.name` / `__name__` over path guesses and falling back to `<module>` when ambiguous.
  - Frames without real filenames (`<string>`, `<frozen ...>`) still cannot produce a meaningful module name. We explicitly document this to prevent support churn.
  - Accessing `sys.modules` must hold the GIL and avoid long-lived Python references; the helper API enforces that discipline.

## Alternatives
- **Keep `<module>` and rely on filenames elsewhere.** Rejected because the filename is not present on every trace consumer surface and is cumbersome for humans.
- **Rewrite module names from filenames only.** Rejected due to incorrect results for namespace packages, zip imports, site-packages bytecode caches, and `.pyc` vs `.py` mismatches.
- **Add a new event field for module name.** Rejected to avoid changing the trace file schema; reusing the existing `function_name` field keeps compat with all tooling.

## References
- `codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs` (`ensure_function_id`).
- `codetracer-python-recorder/src/runtime/tracer/events.rs` (`register_call_record`).
- `codetracer-python-recorder/src/trace_filter/engine.rs` (current module name derivation & caching).
