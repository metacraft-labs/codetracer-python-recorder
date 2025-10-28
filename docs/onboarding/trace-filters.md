# Configurable Trace Filters

## Overview
- Implements user story **US0028 – Configurable Python trace filters** (see `design-docs/US0028 - Configurable Python trace filters.md`).
- Trace filters let callers decide which modules execute under tracing and which values are redacted before the recorder writes events.
- Each filter file is TOML. Files can be chained to layer product defaults with per-project overrides. The runtime records the active filter summary in `trace_metadata.json`.
- The recorder always prepends a built-in **builtin-default** filter that (a) skips CPython standard-library frames (including `asyncio`/concurrency internals) while still allowing third-party packages under `site-packages` (except helper shims like `_virtualenv.py`) and (b) redacts common sensitive identifiers (passwords, tokens, API keys, etc.) across locals/globals/args/returns/attributes. Project filters and explicit overrides append after this baseline and can relax rules where needed.

## Filter Files
- Filters live alongside the project (default: `.codetracer/trace-filter.toml`). Any other file can be supplied via CLI, environment variable, or Python API.
- Required sections:
  - `[meta]` – `name`, `version` (integer), optional `description`.
  - `[scope]` – `default_exec` (`"trace"`/`"skip"`), `default_value_action` (`"allow"`/`"redact"`/`"drop"`).
- Rules appear under `[[scope.rules]]` in declaration order. Each rule has:
  - `selector` – matches a package, file, or object (see selector syntax).
  - Optional `exec` override (`"trace"`/`"skip"`).
  - Optional `value_default` override (`"allow"`/`"redact"`/`"drop"`).
  - Optional `reason` string stored in telemetry.
  - `[[scope.rules.value_patterns]]` entries that refine value capture by selector.
- Example:
  ```toml
  [meta]
  name = "example-filter"
  version = 1
  description = "Protect secrets while allowing metrics."

  [scope]
  default_exec = "trace"
  default_value_action = "allow"

  [[scope.rules]]
  selector = "pkg:my_app.services.*"
  value_default = "redact"
  [[scope.rules.value_patterns]]
  selector = "local:glob:public_*"
  action = "allow"
  [[scope.rules.value_patterns]]
  selector = 'local:regex:^(metric|masked)_\w+$'
  action = "allow"
  [[scope.rules.value_patterns]]
  selector = "local:glob:secret_*"
  action = "redact"
  [[scope.rules.value_patterns]]
  selector = "arg:literal:debug_payload"
  action = "drop"
  ```

## Selector Syntax
- Domains (`selector` prefix before the first colon):
  - `pkg` – fully-qualified module name (`package.module`).
  - `file` – source path relative to the project root (POSIX separators).
  - `obj` – module-qualified object (`package.module.func`).
  - `local`, `global`, `arg`, `ret`, `attr` – value-level selectors.
- Module names normally come from stripping the project root off the code object path. When a filter is inlined (e.g., the builtin defaults) and no filesystem prefix matches, the recorder falls back to `sys.modules`: it scans loaded modules, compares their `__spec__.origin` / `__file__` against the code’s absolute path, and caches the result. This keeps builtin package skips (like `_distutils_hack`) effective even though the filter lives inside the recorder wheel.
- Match types (second segment in `kind:match:pattern`):
  - `glob` *(default)* – wildcard matching with `/` treated as a separator.
  - `regex` – Rust/RE2-style regular expressions; invalid patterns log a single warning and fall back to configuration errors.
  - `literal` – exact string match.
- Value selectors inherit the match type when omitted (e.g., `local:token_*` uses glob). Declare the match type explicitly when combining separators or anchors.

## Loading and Chaining Filters
- Default discovery: `RuntimeTracer` searches for `.codetracer/trace-filter.toml` near the target script.
- CLI: `--trace-filter path/to/filter.toml`. Provide multiple times or use `::` within one argument to append more files.
- Environment: `CODETRACER_TRACE_FILTER=filters/prod.toml::filters/hotfix.toml`. Respected by the auto-start hook and the CLI.
- Python API: `trace(..., trace_filter=[path1, path2])` or pass a `::`-delimited string. Paths are expanded to absolute locations and must exist.
- The recorder loads filters in the order discovered: the built-in `builtin-default` filter first, then project defaults, CLI/env entries, and explicit Python API arguments. Later rules override earlier ones when selectors overlap.

## Runtime Metadata
- `trace_metadata.json` now exposes a `trace_filter` object containing:
  - `filters` – ordered list of filter summaries (`name`, `version`, SHA-256 digest, absolute path).
  - `stats.scopes_skipped` – total number of code objects blocked by `exec = "skip"`.
  - `stats.value_redactions` – per-kind counts for redacted values (`argument`, `local`, `global`, `return`, `attribute`).
  - `stats.value_drops` – per-kind counts for values removed entirely from the trace.
- These counters help CI/quality tooling detect unexpectedly aggressive filters.

## Benchmarks and Guard Rails
- Rust microbench: `cargo bench --bench trace_filter --no-default-features` exercises baseline vs glob/regex-heavy rule sets.
- Python smoke benchmark: `pytest codetracer-python-recorder/tests/python/perf/test_trace_filter_perf.py` runs end-to-end tracing with synthetic workloads when `CODETRACER_TRACE_FILTER_PERF=1`.
- `just bench` orchestrates both:
  1. Ensures the development virtualenv exists (`just venv`).
  2. Runs the Criterion bench with `PYO3_PYTHON` pinned to the virtualenv interpreter.
 3. Executes the Python smoke benchmark, writing `codetracer-python-recorder/target/perf/trace_filter_py.json` (durations plus redaction/drop stats per scenario).
- Use the JSON artefact to feed dashboards or simple regression checks while longer-term gating thresholds are defined.
