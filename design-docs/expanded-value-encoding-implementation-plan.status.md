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
- **WS7 – Rust Fallback & Telemetry:** Completed. `ValueEncoderContext` now
  delegates unsupported values to `PyObject_Repr`, wraps results in a
  `codetracer.repr-fallback` struct with preview, truncation, handler, and
  reason metadata, and surfaces repr failures as `<unrepr>: …` error payloads.
  `ValueFilterStats` records per-kind fallback counts (including truncation
  totals and handler/type breakdowns), and filter summaries emit the new
  telemetry so downstream tooling can prioritise missing handlers.
- **WS8 – Hardening, Benchmarks, and Rollout:** Completed. Captured Criterion
  runs (`target/criterion/trace_filter/workload/*/new/estimates.json`) showing a
  1.07 ms baseline, 8.35 ms regex, and 33.2 ms glob workload in the new encoder,
  and recorded end-to-end Python throughput plus filter telemetry deltas in
  `codetracer-python-recorder/target/perf/trace_filter_py.json`. Updated
  `docs/onboarding/trace-filters.md` with the `value_fallbacks` metadata and
  guidance for interpreting non-zero baseline redaction counts, and refreshed
  `just bench` expectations for sharing results with downstream teams.

## Next tasks
- Coordinate with replay tooling on rollout messaging when the next release is
  cut so `value_fallbacks` dashboards are wired up in parallel.
