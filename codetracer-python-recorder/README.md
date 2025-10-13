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
  --with-diff \
  app/main.py --arg=value
```

- `--trace-dir` (default: `./trace-out`) – directory that will receive
  `trace.json`, `trace_metadata.json`, and `trace_paths.json`.
- `--format` – trace serialisation format (`binary` or `json`). Use `json` for
  integration with the DB backend importer.
- `--activation-path` – optional gate that postpones tracing until the interpreter
  executes this file (defaults to the target script).
- `--with-diff` / `--no-with-diff` – records the caller’s preference in
  `trace_metadata.json`. The desktop Codetracer CLI is responsible for generating
  diff artefacts; the recorder simply surfaces the flag.

All additional arguments are forwarded to the target script unchanged. The CLI
reuses whichever interpreter launches it so wrappers such as `uv run`, `pipx`,
or activated virtual environments behave identically to `python script.py`.

## Packaging expectations

Desktop installers add the wheel to `PYTHONPATH` before invoking the user’s
interpreter. When embedding the recorder elsewhere, ensure the wheel (or its
extracted site-packages directory) is discoverable on `sys.path` and run the CLI
with the interpreter you want to trace.

The CLI writes recorder metadata into `trace_metadata.json` describing the wheel
version, target script, and diff preference so downstream tooling can make
decisions without re-running the trace.
