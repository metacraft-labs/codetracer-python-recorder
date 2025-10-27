# ADR 0016 – Module Name Resolution via `__name__`

- **Status:** Accepted
- **Date:** 2025-03-18
- **Stakeholders:** Runtime team, Trace Filter maintainers
- **Related Decisions:** ADR 0013 (Reliable Module Name Derivation), ADR 0014 (Module Call Event Naming)

## Context

Today both the runtime tracer and the trace filter engine depend on `ModuleIdentityResolver` to derive dotted module names from code-object filenames. The resolver walks `sys.path`, searches `sys.modules`, and applies filesystem heuristics to map absolute paths to module identifiers (`codetracer-python-recorder/src/module_identity.rs:18`). While this produces stable names, it adds complexity to hot code paths (`runtime_tracer.rs:280`) and creates divergence from conventions used elsewhere in the Python ecosystem.

Python’s logging framework, import system, and standard introspection APIs all rely on the module object’s `__name__` attribute. During module execution the `py_start` callback already runs with the module frame active, meaning we can read `frame.f_globals["__name__"]` before we emit events or evaluate filters. This naturally aligns with user expectations and removes the need for filesystem heuristics.

## Decision

We will source module identities directly from `__name__` when a `py_start` callback observes a code object whose qualified name is `<module>`. The runtime tracer will pass that value to both the trace filter coordinator and the event encoder, ensuring call records emit `<{__name__}>` labels and filters match the same string.

Key points:

1. `py_start` inspects the `CodeObjectWrapper` and, for `<module>` qualnames, reads `frame.f_globals.get("__name__")`. If present and non-empty, this name becomes the module identifier for the current scope.
2. Filters always evaluate using the `__name__` value gathered at `py_start`; they no longer attempt to strip file paths or enumerate `sys.modules`.
3. We keep recording the absolute and relative file paths already emitted elsewhere (`TraceFilterEngine::ScopeContext` still normalises filenames for telemetry), but module-name derivation no longer depends on them.
4. We delete `ModuleIdentityResolver`, `ModuleIdentityCache`, and associated heuristics once all call sites switch to the new flow.

## Consequences

**Positive**

- Simplifies hot-path logic, removing `DashMap` lookups and `sys.modules` scans.
- Harmonises trace filtering semantics with Python logging (same strings users already recognise).
- Eliminates filesystem heuristics that were fragile on mixed-case filesystems or when `sys.path` mutated mid-run.

**Negative / Risks**

- Direct scripts (`python my_tool.py`) continue to report `__name__ == "__main__"`. Filters must explicitly target `__main__` in those scenarios, whereas the current resolver maps to `package.module`. We will document the behaviour change and offer guidance for users relying on path-based selectors.
- Synthetic modules created via `runpy.run_path` or dynamic loaders may use ad-hoc `__name__` values. We rely on importers to supply meaningful identifiers, matching Python logging expectations.
- Tests and documentation referencing filesystem-based names must be updated.

**Mitigations**

- Provide a compatibility flag during rollout so filter configurations can opt into the new behaviour incrementally.
- Emit a debug log when `__name__` is missing or empty, falling back to `<module>` exactly as today.
- Preserve path metadata in filter resolutions so existing file-based selectors continue to work.

## Rollout Notes

- Update existing ADR 0013/0014 statuses once this ADR is accepted and the code lands.
- Communicate the behavioural change to downstream teams who consume `<module>` events or rely on path-derived module names.
- Default the `module_name_from_globals` policy to `true` after validation, but retain CLI/env toggles so teams can temporarily fall back to the legacy resolver during rollout.
