# Codetracer Python Recorder

`codetracer-python-recorder` is the Rust-backed recorder module that powers Python
tracing inside Codetracer. The PyO3 extension exposes a small Python façade so
packaged environments (desktop bundles, `uv run`, virtualenvs) can start and stop
recording without shipping an additional interpreter.

## Installation

`codetracer-python-recorder` publishes binary wheels for CPython 3.12 and 3.13 on
Linux (manylinux2014 `x86_64`/`aarch64`), macOS 11+ universal2 (`arm64` + `x86_64`),
and Windows 10+ (`win_amd64`). Install the package into the interpreter you plan to
trace:

```bash
python -m pip install codetracer-python-recorder
```

Source distributions are available for audit and custom builds; maturin and a Rust
toolchain are required when building from source.

## Command-line entry point

The wheel installs a console script named `codetracer-python-recorder` and the
package can also be invoked with `python -m codetracer_python_recorder`. Both
forms share the same arguments:

```bash
python -m codetracer_python_recorder \
  --trace-dir ./trace-out \
  --format json \
  --activation-path app/main.py \
  --trace-filter config/trace-filter.toml \
  app/main.py --arg=value
```

- `--trace-dir` (default: `./trace-out`) – directory that will receive
  `trace.json`, `trace_metadata.json`, and `trace_paths.json`.
- `--format` – trace serialisation format (`binary` or `json`). Use `json` for
  integration with the DB backend importer.
- `--activation-path` – optional gate that postpones tracing until the interpreter
  executes this file (defaults to the target script).
- `--trace-filter` – path to a filter file. Provide multiple times or use `::`
  separators within a single argument to build a chain. When present, the recorder
  prepends the project default `.codetracer/trace-filter.toml` (if found near the
  target script) so later entries override the defaults. The
  `CODETRACER_TRACE_FILTER` environment variable accepts the same `::`-separated
  syntax when using the auto-start hook.

All additional arguments are forwarded to the target script unchanged. The CLI
reuses whichever interpreter launches it so wrappers such as `uv run`, `pipx`,
or activated virtual environments behave identically to `python script.py`.

## Trace filter configuration
- Filter files are TOML with `[meta]`, `[scope]`, and `[[scope.rules]]` tables. Rules evaluate in declaration order and can tweak both execution (`exec`) and value decisions (`value_default`).
- Supported selector domains: `pkg`, `file`, `obj` for scopes; `local`, `global`, `arg`, `ret`, `attr` for value policies. Match types default to `glob` and also accept `regex` or `literal` (e.g. `local:regex:^(metric|masked)_\w+$`).
- Default discovery: `.codetracer/trace-filter.toml` next to the traced script. Chain additional files via CLI (`--trace-filter path_a --trace-filter path_b`), environment variable (`CODETRACER_TRACE_FILTER=path_a::path_b`), or Python helpers (`trace(..., trace_filter=[path_a, path_b])`). Later entries override earlier ones when selectors overlap.
- A built-in `builtin-default` filter is always prepended. It skips CPython standard-library frames (e.g. `asyncio`, `threading`, `importlib`) while re-enabling third-party packages under `site-packages` (except helpers such as `_virtualenv.py`), and redacts common secrets (`password`, `token`, API keys, etc.) across locals/globals/args/returns/attributes. Project filters can loosen or tighten these defaults as required.
- Runtime metadata captures the active chain under `trace_metadata.json -> trace_filter`, including per-kind redaction and drop counters. See `docs/onboarding/trace-filters.md` for the full DSL reference and examples.

Example snippet:
```toml
[meta]
name = "local-redaction"
version = 1

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
selector = "arg:literal:debug_payload"
action = "drop"
```

## Trace naming semantics

- Module-level activations no longer appear as the ambiguous `<module>` label. When the recorder sees `co_qualname == "<module>"`, it first reuses the frame's `__name__`, then falls back to trace-filter hints, `sys.path` roots, and package markers so scripts report `<__main__>` while real modules keep their dotted names (e.g., `<my_pkg.mod>` or `<boto3.session>`).
- The globals-derived naming flow ships enabled by default; disable it temporarily with `--no-module-name-from-globals`, `codetracer.configure_policy(module_name_from_globals=False)`, or `CODETRACER_MODULE_NAME_FROM_GLOBALS=0` if you need to compare against the legacy resolver.
- The angle-bracket convention remains for module entries so downstream tooling can distinguish top-level activations at a glance.
- Traces will still emit `<module>` for synthetic filenames (`<stdin>`, `<string>`), frozen/importlib bootstrap frames, or exotic loaders that omit filenames entirely. This preserves previous behaviour when no reliable name exists.

## Packaging expectations

Desktop installers add the wheel to `PYTHONPATH` before invoking the user’s
interpreter. When embedding the recorder elsewhere, ensure the wheel (or its
extracted site-packages directory) is discoverable on `sys.path` and run the CLI
with the interpreter you want to trace.

The CLI writes recorder metadata into `trace_metadata.json` describing the wheel
version, target script, and diff preference so downstream tooling can make
decisions without re-running the trace.

## Development benchmarks
- Rust microbench: `cargo bench --bench trace_filter --no-default-features` exercises baseline, glob-heavy, and regex-heavy selector chains.
- Python smoke benchmark: `pytest codetracer-python-recorder/tests/python/perf/test_trace_filter_perf.py -q` when the environment variable `CODETRACER_TRACE_FILTER_PERF=1` is set.
- Run both together with `just bench`. The helper seeds a virtualenv, runs Criterion, then executes the Python smoke test while writing `target/perf/trace_filter_py.json` (per-scenario durations plus redaction/drop statistics).
