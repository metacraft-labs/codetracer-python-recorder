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
- **WS4 – Rust Text, Binary, and Paths:** Completed. Strings now enforce a
  256-character preview budget and fall back to a `codetracer.string-preview`
  struct when truncated, bytes-like objects (bytes/bytearray/memoryview) emit
  base64 previews via `codetracer.bytes-preview`, and path-like values pass
  through `os.fspath()` while registering stable `pathlib.*` type ids. New
  fixtures (`text_binary.json`) exercise the preview metadata and the parity
  suite confirms Python/Rust alignment.
- **WS5 – Rust Collections & Recursion Management:** Completed. Sets and
  frozensets surface as `codetracer.set-metadata` structs with preview budgets,
  ranges encode start/stop/step via `codetracer.range`, and general iterables
  such as `collections.deque` respect breadth limits during traversal. Fresh
  fixtures (`sets.json`, `ranges.json`, `collections.json`) plus unit tests for
  metadata ensure the Rust encoder and Python parity harness agree on the new
  shapes.
- **WS6 – Rust Structured & Temporal Types:** Completed. Dataclasses, attrs
  classes, namedtuples, enums, and `types.SimpleNamespace` now encode as
  `ValueRecord::Struct` with stable field metadata, while `datetime`
  primitives (datetime/date/time/timedelta/timezone) emit ISO strings plus raw
  components. Supporting fixtures (`structured.json`, `temporal.json`) and
  unit tests keep the Rust encoder and Python parity suite aligned.
- WS7–WS8: Not started.

## Next tasks
- WS1: Fold the preview/set metadata semantics into `encode-values.md` so the
  contract examples mirror the text/binary/collection fixtures.
- WS2: Flesh out reference-emission strategy (`ValueRecord::Reference`) ahead
  of WS6 so cyclic linked structures share breadth limits across handlers.
- WS3: Add fixture coverage for special float/decimal cases (NaN, Infinity,
  quantised decimals) and capture regressions via property tests.
- WS4: Extend preview coverage to exercise surrogate pairs and zero-length
  payloads, then benchmark the impact of the new limits.
- WS5: Audit breadth budgeting on deeply nested mixed containers now that
  structured types are covered, ensuring metadata remains bounded.
- WS6: Fold the structured/temporal encoding semantics into
  `design-docs/encode-values.md` and extend fixtures to cover edge cases (e.g.,
  zero-offset timezones, dataclasses with defaults).
- Prepare WS7 design work (repr fallback & telemetry) leveraging the enlarged
  registry metadata helpers.
