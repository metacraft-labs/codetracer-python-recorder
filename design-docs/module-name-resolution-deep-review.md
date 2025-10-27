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

- `codetracer-python-recorder/src/module_identity.rs` – lightweight helpers such as `module_from_relative`, `module_name_from_packages`, and `normalise_to_posix` that turn paths into dotted names.
- `codetracer-python-recorder/src/trace_filter/engine.rs` – builds a `ScopeContext`, keeps relative/absolute paths, and applies package-aware heuristics whenever filters do not provide an explicit module hint.
- `codetracer-python-recorder/src/runtime/tracer/runtime_tracer.rs` – turns `<module>` frames into `<{module}>` labels by preferring the globals-derived hint, falling back to filter resolutions, and finally to the on-disk package structure.
- `codetracer-python-recorder/src/runtime/tracer/filtering.rs` – captures filter outcomes, stores module hints collected during `py_start`, and exposes them to the tracer.
- Tests exercising the behaviour: Rust unit tests in `codetracer-python-recorder/src/module_identity.rs` and Python integration coverage in `codetracer-python-recorder/tests/python/test_monitoring_events.py`.

The pure-Python recorder does not perform advanced module-name derivation; all reusable logic lives in the Rust-backed module.

## 5. How the Algorithm Works End to End

### 5.1 Capturing module hints
1. `RuntimeTracer::on_py_start` grabs `frame.f_globals['__name__']` for `<module>` code objects and stores that value in `FilterCoordinator` before any gating decisions run.
2. The hint is retained for the lifetime of the code object (and cleared once non-module frames arrive) so both the filter engine and the tracer can reuse it.

### 5.2 Filter resolution
1. `TraceFilterEngine::resolve` begins with configuration metadata: project-relative paths, activation roots, and module names supplied by filters.
2. If no valid module name is present, the engine consults the incoming hint. Failing that, it walks up the filesystem looking for `__init__.py` packages (`module_name_from_packages`) and uses the file stem as a last resort.
3. The resulting `ScopeResolution` records the derived module name, relative and absolute paths, and the execution decision; the resolution is cached per code object.

### 5.3 Runtime naming
1. When emitting events, `RuntimeTracer::function_name` first checks the stored module hint. If the globals-derived name exists, it wins; this keeps the default behaviour aligned with Python logging while still permitting explicit opt-outs.
2. Absent a hint, the tracer falls back to the cached `ScopeResolution` module name, then to package detection via `module_name_from_packages`, and finally leaves `<module>` unchanged.
3. The tracer no longer keeps a resolver cache, so the hot path is reduced to string comparisons and light filesystem checks.

## 6. Bugs, Risks, and Observations

- **Globals may remain `__main__` for direct scripts** – this is intentional and matches logging, but filter authors must target `pkg:__main__` when skipping script bodies.
- **Package detection relies on `__init__.py`** – namespace packages without marker files produce the leaf module name only (e.g., `service.handler`). If this proves problematic we can detect `pyproject.toml`/`setup.cfg` to extend coverage.
- **Filesystem traversal on first encounter** – the package walk performs existence checks until it finds the first non-package directory. Results are cached per code object so the overhead is modest, but tracing thousands of unique modules on slow filesystems could still be noticeable.
- **Globals introspection can fail** – exotic frames that refuse `PyFrame_FastToLocalsWithError` leave the hint empty. We fall back to filesystem heuristics, but selectors relying solely on globals may miss until the module imports cleanly.

These trade-offs are significantly narrower than the previous resolver-based design and keep the module-name derivation consistent with Python's own conventions.
