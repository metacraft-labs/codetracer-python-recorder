## CodeTracer Recorders (Monorepo)

This repository now hosts two related projects:

- codetracer-pure-python-recorder — a pure-Python tracer that still mirrors the early prototype.
- codetracer-python-recorder — a Rust-backed Python extension (PyO3 + maturin) with structured errors and tighter tooling.

Both projects are still in motion. Expect breaking changes while we finish the error-handling rollout.

### Structured errors (Rust-backed recorder)

The Rust module wraps every failure in a `RecorderError` hierarchy that reaches Python with a stable `code`, a readable `kind`, and a `context` dict.

- `UsageError` → bad input or calling pattern. Codes like `ERR_ALREADY_TRACING`.
- `EnvironmentError` → IO or OS problems. Codes like `ERR_IO`.
- `TargetError` → the traced program raised or refused inspection. Codes like `ERR_TRACE_INCOMPLETE`.
- `InternalError` → a recorder bug or panic. Codes default to `ERR_UNKNOWN` unless classified.

Quick catch example:

```python
from codetracer_python_recorder import RecorderError, start, stop

try:
    session = start("/tmp/trace", format="json")
except RecorderError as err:
    print(f"Recorder failed: {err.code}")
    for key, value in err.context.items():
        print(f"  {key}: {value}")
else:
    try:
        ...  # run work here
    finally:
        session.flush()
        stop()
```

All subclasses carry the same attributes, so existing handlers can migrate by catching `RecorderError` once and branching on `err.code` if needed.

### CLI exit behaviour and JSON trailers

`python -m codetracer_python_recorder` returns:

- `0` when tracing and the target script succeed.
- The script's own exit code when it calls `sys.exit()`.
- `1` when a `RecorderError` bubbles out of startup or shutdown.
- `2` when the CLI arguments are incomplete.

Pass `--codetracer-json-errors` (or set the policy via `configure_policy(json_errors=True)`) to stream a one-line JSON trailer on stderr. The payload includes `run_id`, `trace_id`, `error_code`, `error_kind`, `message`, and the `context` map so downstream tooling can log failures without scraping text.

### Migration checklist for downstream tools

- Catch `RecorderError` (or a subclass) instead of `RuntimeError`.
- Switch any string matching over to `err.code` values like `ERR_TRACE_DIR_CONFLICT`.
- Expect structured log lines (JSON) on stderr. Use the `error_code` field instead of parsing text.
- Opt in to JSON trailers for machine parsing and keep human output short.
- Update policy wiring to use `configure_policy` / `policy_snapshot()` rather than hand-rolled env parsing.
- Read `docs/onboarding/error-handling.md` for detailed migration steps and assertion rules.

### Logging defaults

The recorder now installs a JSON logger on first import. Logs include `run_id`, optional `trace_id`, and an `error_code` field when set.

- Control the log filter with `RUST_LOG=target=level` (standard env syntax).
- Override from Python with `configure_policy(log_level="info")` or `log_file=...` for file output.
- Metrics counters record dropped events, detach reasons, and caught panics; plug your own sink via the Rust API when embedding.

### codetracer-pure-python-recorder

Install from PyPI:

```bash
pip install codetracer-pure-python-recorder
```

CLI usage:

```bash
codetracer-record <path to python file>
# produces several trace json files in the current directory
# or in the folder of `$CODETRACER_DB_TRACE_PATH` if such an env var is defined
```

During development you can also run it directly:

```bash
python src/trace.py <path to python file>
# produces several trace json files in the current directory
# or in the folder of `$CODETRACER_DB_TRACE_PATH` if such an env var is defined
```

### codetracer-python-recorder (Rust-backed)

A separate Python module implemented in Rust with PyO3 and built via maturin lives under:
crates/codetracer-python-recorder/

Basic workflow:

- Build/dev install the Rust module:
  - maturin develop -m crates/codetracer-python-recorder/Cargo.toml
- Use in Python:
  - from codetracer_python_recorder import hello
  - hello()

#### Testing & Coverage

- Run the full split test suite (Rust nextest + Python pytest): `just test`
- Run only Rust integration/unit tests: `just cargo-test`
- Run only Python tests (including the pure-Python recorder to guard regressions): `just py-test`
- Collect coverage artefacts locally (LCOV + Cobertura/JSON): `just coverage`

The CI workflow mirrors these commands. Pull requests get an automated comment with the latest Rust/Python coverage tables and downloadable artefacts (`lcov.info`, `coverage.xml`, `coverage.json`).

#### Debug logging

Rust-side logging defaults to `warn` so test output stays readable. Export
`RUST_LOG` when you need more detail:

```bash
RUST_LOG=codetracer_python_recorder=debug pytest \
  codetracer-python-recorder/tests/python/unit/test_backend_exceptions.py -q
```

Any filter accepted by `env_logger` still works, so you can switch to
`RUST_LOG=codetracer_python_recorder=info` or silence everything with
`RUST_LOG=off`.

### Future directions

The current Python support is an unfinished prototype. We can finish it. In the future, it may be expanded to function in a way to similar to the more complete implementations, e.g. [Noir](https://github.com/blocksense-network/noir/tree/blocksense/tooling/tracer).

Currently it's very similar to our [Ruby tracer](https://github.com/metacraft-labs/ct-ruby-tracer)

#### Current approach: sys.settrace API

Currently we're using the sys.settrace API: https://docs.python.org/3/library/sys.html#sys.settrace .
This is very flexible and can function with probably multiple Python versions out of the box. 
However, this is limited:

* it's not optimal
* it can't track more detailed info/state, needed for some CodeTracer features(or for more optimal replays).

For other languages, we've used a more deeply integrated approach: patching the interpreter or VM itself (e.g. Noir).

#### Patching the VM

This can be a good approach for Python as well: it can let us record more precisely subvalues, assignments and subexpressions and to let
some CodeTracer features work in a deeper/better way.

One usually needs to add additional logic to places where new opcodes/lines are being ran, and to call entries/exits. Additionally
tracking assignments can be a great addition, but it really depends on the interpreter internals.

#### Filtering

It would be useful to have a way to record in detail only certain periods of the program, or certain functions or modules: 
we plan on expanding the [trace format](https://github.com/metacraft-labs/runtime_tracing/) and CodeTracer' support, so that this is possible. It would let one be able to record interesting
parts of even long-running or more heavy programs.

### Contributing

We'd be very happy if the community finds this useful, and if anyone wants to:

* Use and test the Python support or CodeTracer.
* Provide feedback and discuss alternative implementation ideas: in the issue tracker, or in our [discord](https://discord.gg/qSDCAFMP).
* Contribute code to enhance the Python support of CodeTracer.
* Provide [sponsorship](https://opencollective.com/codetracer), so we can hire dedicated full-time maintainers for this project.

### Legal info

LICENSE: MIT

Copyright (c) 2025 Metacraft Labs Ltd
