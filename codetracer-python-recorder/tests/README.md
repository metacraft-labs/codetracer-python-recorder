# Test Layout

This crate now keeps all integration-style tests under a single `tests/` root so
developers can see which harness they are touching at a glance.

- `python/` — Pytest and unittest suites that exercise the public Python API and
  high-level tracing flows. Invoke with `uv run --group dev --group test pytest
  codetracer-python-recorder/tests/python`.
- `rust/` — Rust integration tests that embed CPython through PyO3. These are
  collected via the `tests/rust.rs` aggregator and run with `uv run cargo nextest
  run --manifest-path codetracer-python-recorder/Cargo.toml --no-default-features`.
- Shared fixtures and helpers will live under `tests/support/` as they are
  introduced in later stages of the improvement plan.

For unit tests that do not require the FFI boundary, prefer `#[cfg(test)]`
modules co-located with the Rust source, or Python module-level tests inside the
`codetracer_python_recorder` package.

## Coverage Workflow

When you need coverage artefacts, prefer the Just helpers so local runs match the
CI configuration:

- `just coverage-rust` runs `cargo llvm-cov nextest` with the same flags as the
  Rust test job and writes `lcov.info` under
  `codetracer-python-recorder/target/coverage/rust/`. The helper wires in
  `LLVM_COV`/`LLVM_PROFDATA` from the dev shell (provided by
  `llvmPackages_latest.llvm`), so make sure you re-enter `nix develop` after
  pulling updates. It also captures `summary.json` and prints a per-file table
  so the console mirrors the pytest coverage output. (Run
  `cargo llvm-cov nextest --html` manually if you need a browsable report.)
- `just coverage-python` executes pytest with `pytest-cov`, limiting collection to
  `codetracer_python_recorder` and emitting both `coverage.xml` and `coverage.json`
  in `codetracer-python-recorder/target/coverage/python/`.
- `just coverage` is a convenience wrapper that invokes both commands in sequence.

All commands create their output directories on first run, so no manual setup is
required beyond entering the Nix shell (`nix develop`) or syncing the UV virtual
environment (`just venv`).
