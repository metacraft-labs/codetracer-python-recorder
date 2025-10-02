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
