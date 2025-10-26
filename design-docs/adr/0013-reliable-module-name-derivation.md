# ADR 0013: Reliable Module Name Derivation for Trace Filters

- **Status:** Proposed
- **Date:** 2025-03-15
- **Deciders:** codetracer recorder maintainers
- **Consulted:** Python runtime SMEs, DX/observability stakeholders
- **Informed:** Support engineers, product analytics consumers

## Context
- Scope rules in the configurable trace filter engine match code objects using package (`pkg:*`), file, and object selectors.
- Package selectors require the engine to derive a module name from `code.co_filename`. We currently guess the module by stripping each filter source’s `project_root` from the absolute filename and converting the remainder to dotted form (`ScopeContext::derive`).
- The builtin filter is injected as an inline source (`<inline:builtin-default>`) whose `project_root` resolves to `"."`. That path is not a prefix of system libraries (e.g., `/usr/lib/python3.12/site-packages/_distutils_hack/__init__.py`), so module derivation fails and the code’s `module_name` stays `None`.
- When module name derivation fails, package selectors silently never match—even though configuration authors expect builtin skips (such as `_distutils_hack`) to work regardless of how the filter is loaded.

## Problem
- Inline filters and filters stored outside the traced project cannot derive module names, causing all `pkg:*` selectors from those sources to be ignored.
- Users cannot observe or correct this: they only see that builtin skip rules or inline filters “do nothing,” which looks like a bug and pollutes traces with unwanted modules.
- Relying solely on relative paths ties filter correctness to the filesystem layout, which is fragile for virtual environments, zip apps, or global site-packages.

## Decision
1. **Augment module derivation with `sys.modules`.**
   - Keep the existing relative-path heuristic for performance when it succeeds.
   - When it fails to produce a module name, fall back to scanning `sys.modules` for entries whose `__file__` matches the canonicalised `co_filename`.
   - Cache the mapping from absolute filename to module name inside `ScopeContext` (or the engine) to avoid repeated scans on hot code paths.
2. **Expose the derived module even for inline/builtin filters.**
   - `pkg:*` selectors attached to inline sources should now match by module name, independent of project roots.
3. **Retain current behaviour for synthetic filenames.**
   - Frames with `<string>` or `<frozen ...>` filenames still produce no module, so package selectors continue to skip them.
4. **Surface diagnostics when both derivation strategies fail.**
   - Add a trace-level log so users understand why a `pkg:*` selector may not hit, aiding troubleshooting.

## Consequences
- **Pros**
  - Builtin skip rules (e.g., `_distutils_hack`) start working immediately, improving trace signal/noise without extra user configuration.
  - Inline filters defined via CLI, env vars, or API behave identically to on-disk filters, aligning with user expectations.
  - Reusing Python’s own module registry removes the guesswork around path prefixes, making the system robust to virtualenv or zipapp layouts.
- **Cons**
  - Scanning `sys.modules` is O(n) in the number of loaded modules, so we must cache results and only fall back when necessary.
  - Module derivation now depends on `sys.modules` entries having accurate `__file__` attributes; exotic loaders that omit them will still fail (but that is already true for path-based detection).
- **Risks**
  - The fallback introduces Python interaction inside the resolution path; bugs here could deadlock if we mishandle the GIL or degrade performance.
  - Module caching must be invalidated when module files are reloaded from different paths; we assume trace sessions do not mutate modules aggressively.

## Alternatives
- **Require all filters to live under the traced project root.** Rejected: impossible for builtin filters and unreasonable for global hooks.
- **Add explicit annotations to trace metadata with module names provided by the target script.** Rejected: burdens users and still fails for builtin filters.
- **Ignore package selectors for inline filters.** Rejected: contradicts documented behaviour and leaves builtin skips ineffective.

## References
- `codetracer-python-recorder/src/trace_filter/engine.rs` (`ScopeContext::derive` implementation).
- Python documentation for `sys.modules` and module attributes: https://docs.python.org/3/library/sys.html#sys.modules
