# Expanded Value Encoding Implementation Plan Status

## Key Reference documents
- ADR 0018 — Expanded value encoding coverage with repr fallback
- design-docs/encode-values.md — Value encoding contract
- design-docs/expanded-value-encoding-implementation-plan.md

## Key Source Files
- codetracer-python-recorder/src/runtime/value_encoder.rs
- codetracer-python-recorder/src/runtime/value_capture.rs
- codetracer-python-recorder/tests/data/values/ (planned fixtures)
- runtime_tracing crate (ValueRecord definitions)

## Current Status
- **WS1 – Shared Contract & Fixtures:** In progress. Updated
  `design-docs/encode-values.md` to capture the Rust recorder contract,
  including truncation metadata, type identifiers, and repr fallback guidance.
  Fixture directory earmarked but fixtures and automated verifiers still pending.
- WS2–WS8: Not started.

## Next tasks
- WS1: Add initial golden fixtures under
  `codetracer-python-recorder/tests/data/values/` and hook them into the Rust
  and Python test suites.
- WS1: Introduce a lightweight verifier that reads the fixtures and asserts
  encoder output parity.
- Prepare WS2 design notes (registry shape, handler traits) once fixture
  harness is in place.
