# Expanded Value Encoding Implementation Plan Status

## Key Reference documents
- ADR 0018 — Expanded value encoding coverage with repr fallback
- design-docs/encode-values.md — Value encoding contract
- design-docs/expanded-value-encoding-implementation-plan.md

## Key Source Files
- codetracer-python-recorder/src/runtime/value_encoder.rs
- codetracer-python-recorder/src/runtime/value_filters.rs
- codetracer-python-recorder/src/runtime/value_capture.rs
- codetracer-python-recorder/tests/data/values/basic.json
- codetracer-python-recorder/tests/data/values/advanced.json
- runtime_tracing crate (ValueRecord definitions)

## Current Status
- **WS1 – Shared Contract & Fixtures:** Completed. Contract doc refreshed, the
  JSON fixtures normalise multi-line snippets, the Rust harness
  (`value_encoding_fixtures_match_contract`) canonicalises type ids, and a new
  integration-test helper (`encode_value_fixture`) powers the pytest parity
  suite (`tests/python/unit/test_value_encoding_contract.py`). `just dev test`
  now exercises both Rust and Python harnesses without additional environment
  tweaks.
- **WS2 – Rust Encoder Registry & Guardrails:** Completed. Introduced a
  registry-driven `ValueEncoderContext` with depth/object budgets, extracted a
  reusable policy adapter (`runtime/value_filters.rs`), and added targeted unit
  tests for handler ordering and recursion budgeting. Existing behaviour is
  preserved with the new architecture wired through both Rust and Python
  parity checks; the reference-emission tests now target PyO3 0.25's
  `PyList::new`/`PyTuple::new` API so `just dev test` stays green.
- **WS3 – Rust Scalars & Numerics:** Completed. Booleans, ints, floats, and
  string handlers now emit module-qualified type ids, python ints larger than
  `i64` fall back to `ValueRecord::BigInt`, and new handlers cover floats,
  complex numbers, `decimal.Decimal`, and `fractions.Fraction` with struct
  metadata. Expanded fixtures (`numerics.json`) and dedicated unit tests ensure
  big-int aliasing, tuple encoding for complex values, and struct field
  ordering stay stable across the Rust/Python parity suites.
- WS4–WS8: Not started.

## Next tasks
- WS1: Add fixture coverage for binary payload previews/truncation once the
  encoder feature lands.
- WS2: Flesh out reference-emission strategy (`ValueRecord::Reference`) and
  begin wiring breadth budgets once handlers start covering additional types in
  WS4.
- WS3: Add fixture coverage for special float/decimal cases (NaN, Infinity,
  quantised decimals) and capture regressions via property tests.
- Prepare WS4 design work (text/binary handlers) using the expanded registry.
