# Module Name Resolution Deep Review

This document walks through how CodeTracer derives dotted module names from running Python code, why the problem matters, and where the current implementation may need refinement. The goal is to make the topic approachable for readers who only have basic familiarity with Python modules.

## 1. What Problem Are We Solving?

Python reports the top-level body of any module with the generic qualified name `<module>`. When CodeTracer records execution, every package entry point therefore looks identical unless we derive the real dotted module name ourselves (for example, turning `<module>` into `<mypkg.tools.cli>`). Without that mapping:

- trace visualisations lose context because multiple files collapse to `<module>`
- trace filters cannot match package-level rules (e.g., `pkg:glob:mypkg.*`)
- downstream tooling struggles to correlate events back to the source tree

Our recorder must therefore infer the right module name from runtime metadata, file paths, and user configuration, even in tricky situations such as site-packages code, editable installs, or scripts executed directly.

## 2. Python Module Background (Minimal Primer)

The current design assumes the reader understands the following basics:

- **Modules and packages** – every `.py` file is a module; a directory with an `__init__.py` is treated as a package whose submodules use dotted names like `package.module`.
- **`sys.path`** – Python searches each entry (directories or zip files) when importing modules. Joining a path entry with the relative file path yields the dotted name.
- **`sys.modules`** – a dictionary of module objects keyed by dotted module name. Each module typically exposes `__spec__`, `__name__`, and `__file__`, which reveal how and from where it was loaded.
- **Code objects** – functions and module bodies have code objects whose `co_filename` stores the source path and whose `co_qualname` stores the qualified name (`<module>` for top-level bodies).
- **Imports are idempotent** – when Python imports a module it first creates and registers the module object in `sys.modules`, then executes its body. That guarantees CodeTracer can query `sys.modules` while tracing the import.

## 3. Why CodeTracer Resolves Module Names

- **Trace event labelling** – `RuntimeTracer::function_name` converts `<module>` frames into `<dotted.package>` so traces remain readable (`codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs:280`).
- **Trace filter decisions** – package selectors rely on module names, and file selectors reuse the same normalised paths. The filter engine caches per-code-object decisions together with the resolved module identity (`codetracer-python-recorder/src/trace_filter/engine.rs:183` and `:232`).
- **File path normalisation** – both subsystems need consistent POSIX-style paths for telemetry, redaction policies, and for emitting metadata in trace files.
- **Cross-component hints** – filter resolutions capture the module name, project-relative path, and absolute path and hand them to the runtime tracer so both parts agree on naming (`codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs:292`).

## 4. Where the Logic Lives

- `codetracer-python-recorder/src/module_identity.rs:18` – core resolver, caching, heuristics, and helpers (`normalise_to_posix`, `module_from_relative`, etc.).
- `codetracer-python-recorder/src/trace_filter/engine.rs:183` – owns a `ModuleIdentityResolver` instance, builds `ScopeContext`, and stores resolved module names inside `ScopeResolution`.
- `codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs:280` – turns `<module>` frames into `<{module}>` labels using `ModuleIdentityCache`.
- `codetracer-python-recorder/src/runtime/tracer/filtering.rs:19` – captures filter outcomes and pipes their module hints back to the tracer.
- Tests exercising the behaviour: Rust unit tests in `codetracer-python-recorder/src/module_identity.rs:551` and Python integration coverage in `codetracer-python-recorder/tests/python/test_monitoring_events.py:199`.

The pure-Python recorder does not perform advanced module-name derivation; all of the reusable logic lives in the Rust-backed module.

## 5. How the Algorithm Works End to End

### 5.1 Building the resolver
1. `ModuleIdentityResolver::new` snapshots `sys.path` under the GIL, normalises each entry to POSIX form, removes duplicates, and sorts by descending length so more specific roots win (`module_identity.rs:24`–`:227`).
2. Each entry stays cached for the duration of the recorder session; mutations to `sys.path` after startup are not observed automatically.

### 5.2 Resolving an absolute filename
Given a normalised absolute path (e.g., `/home/app/pkg/service.py`):

1. **Path-based attempt** – `module_name_from_roots` strips each known root and converts the remainder into a dotted form (`pkg/service.py → pkg.service`). This is fast and succeeds for project code and site-packages that live under a `sys.path` entry (`module_identity.rs:229`–`:238`).
2. **Heuristics** – if the first guess looks like a raw filesystem echo (meaning we probably matched a catch-all root like `/`), the resolver searches upward for project markers (`pyproject.toml`, `.git`, etc.) and retries with that directory as the root. Failing that, it uses the immediate parent directory (`module_identity.rs:240`–`:468`).
3. **`sys.modules` sweep** – the resolver iterates through loaded modules, comparing `__spec__.origin`, `__file__`, or `__cached__` paths (normalised to POSIX) against the target filename, accounting for `.py` vs `.pyc` differences. Any valid dotted name wins over heuristic guesses (`module_identity.rs:248`–`:335`).
4. The winning name (or lack thereof) is cached by absolute path in a `DashMap` so future lookups avoid repeated sys.modules scans (`module_identity.rs:54`–`:60`).

### 5.3 Mapping code objects to module names

`ModuleIdentityCache::resolve_for_code` accepts optional hints:

1. **Preferred hint** – e.g., the filter engine’s stored module name. It is accepted only if it is a valid dotted identifier (`module_identity.rs:103`–`:116`).
2. **Relative path** – converted via `module_from_relative` when supplied (`module_identity.rs:107`–`:110`).
3. **Absolute path** – triggers the resolver described above (`module_identity.rs:112`–`:115`).
4. **Globals-based hint** – a last resort using `frame.f_globals["__name__"]` when available (`module_identity.rs:116`).

If no hints contain an absolute path, the cache will read `co_filename`, normalise it, and resolve it once (`module_identity.rs:118`–`:127`). Results (including failures) are memoised per `code_id`.

### 5.4 Feeding results into the rest of the system

1. The trace filter builds a `ScopeContext` for every new code object. It records project-relative paths and module names derived from configuration roots, then calls back into the resolver if the preliminary name is missing or invalid (`trace_filter/engine.rs:420`–`:471`).
2. The resulting `ScopeResolution` is cached and exposed to the runtime tracer via `FilterCoordinator`, providing rich hints (`runtime/tracer/filtering.rs:43`).
3. During execution, `RuntimeTracer::function_name` reaches into the shared cache to turn `<module>` qualnames into `<pkg.module>` labels. If every heuristic fails, it safely falls back to the original `<module>` string (`runtime_tracer.rs:280`–`:305`).
4. Both subsystems reuse the same `ModuleIdentityResolver`, ensuring trace files and filtering decisions stay consistent.

## 6. Bugs, Risks, and Observations

- **Prefix matching ignores path boundaries** – `strip_posix_prefix` checks `path.starts_with(base)` without verifying the next character is a separator. A root like `/opt/app` therefore incorrectly matches `/opt/application/module.py`, yielding the bogus module `lication.module`. When `sys.modules` lacks the correct entry (e.g., resolving a file before import), the resolver will cache this wrong answer (`module_identity.rs:410`–`:426`).
- **Case sensitivity on Windows** – normalisation preserves whatever casing the OS returns. If `co_filename` and `sys.modules` report the same path with different casing, `equivalent_posix_paths` will not treat them as equal, causing the fallback to miss (`module_identity.rs:317`–`:324`). Consider lowercasing drive prefixes or using `Path::eq` semantics behind the GIL.
- **`sys.path` mutations after startup** – the resolver snapshots roots once. If tooling modifies `sys.path` later (common in virtualenv activation scripts), we will never see the new prefix, so we fall back to heuristics or `sys.modules`. Documenting this behaviour or exposing a method to refresh the roots may avoid surprises.
- **Project-marker heuristics hit the filesystem** – `has_project_marker` calls `exists()` for every parent directory when the fast path fails (`module_identity.rs:470`–`:493`). Because results are cached per file this is usually acceptable, but tracing thousands of unique `site-packages` files on network storage could still become expensive.

Addressing the first two items would materially improve correctness; the latter two are design trade-offs worth monitoring.

