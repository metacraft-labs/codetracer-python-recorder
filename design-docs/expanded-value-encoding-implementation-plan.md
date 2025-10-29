# Expanded Value Encoding Implementation Plan

Deliver the strategy described in ADR 0018: refresh the encoding contract,
broaden structured coverage inside the Rust recorder, and adopt `repr`-based
fallback while respecting the refactored tracing pipeline.

## Goals
- Refresh `design-docs/encode-values.md` into a single source of truth for value
  shapes, truncation metadata, and type identifiers.
- Replace the ad-hoc cascade in
  `codetracer-python-recorder/src/runtime/value_encoder.rs` with an extensible,
  policy-aware registry that enforces recursion and breadth limits.
- Switch the Rust recorder’s fallback to `repr`, surfacing failures as
  structured errors without bypassing `ValuePolicy` redaction/drop decisions.
- Provide comprehensive fixtures, unit tests, and property checks that prevent
  regressions and quantify throughput/latency impact.

## Current Gaps
- `encode_value` recognises only `None`, `bool`, 64-bit `int`, `str`,
  tuples, lists, and dicts; everything else uses `value.str()`. Large integers,
  floats, binary payloads, temporal types, and user-defined objects lose
  structure.
- No fixtures validate encoder behaviour end-to-end; the legacy
  `design-docs/encode-values.md` does not reflect current behaviour.
- Recursion is unchecked: self-referential containers trigger unbounded calls,
  and there is no truncation metadata for large collections or byte blobs.
- `ValuePolicy` emits `<redacted>`/`<dropped>` sentinels via
  `runtime::value_capture`, but the encoder cannot flag partially captured data
  or report which handler produced a fallback.

## Workstreams

### WS1 – Shared Contract & Fixtures
**Scope:** Provide authoritative documentation and parity fixtures.
- Rewrite `design-docs/encode-values.md` to match ADR 0018’s contract (type
  names, recursion limits, truncation fields, error semantics).
- Create golden fixtures (`codetracer-python-recorder/tests/data/values/*.json`)
  capturing expected `ValueRecord` shapes for scalars, collections, temporal
  values, structured objects, and fallback cases.
- Add a lightweight verifier in Rust (unit test module) and Python (pytest) that
  loads the fixtures and asserts encoder output equality.

**Exit criteria:** Contract doc merged; fixtures exercised in CI without diffs.

### WS2 – Rust Encoder Registry & Guardrails
**Scope:** Introduce infrastructure that future handlers can share.
- Replace the direct cascade in `runtime/value_encoder.rs` with a registry of
  handlers owning guard predicates, encode callbacks, and type-registration
  helpers. Store shared state (seen object ids, depth/breadth budgets, emitted
  elements) in a dedicated context struct.
- Move redaction-aware helpers (`ValuePolicy`, sentinel handling) into a small
  adapter so handlers only deal with encoding logic.
- Document the registry contract in module docs and add targeted unit tests for
  guard ordering and recursion limits.

**Exit criteria:** Existing behaviour reproduced via the registry with parity
tests; depth/breadth budgets configurable via constants.

### WS3 – Rust Scalars & Numerics
**Scope:** Extend coverage for numeric and scalar types.
- Encode arbitrary-size integers with best-effort downcasting to `i128`/`u128`
  before falling back to struct representations (sign + digits) and ensure
  deterministic type ids (e.g., `builtins.int`).
- Handle `float`/`complex` (including `NaN`/`Inf`), `decimal.Decimal`, and
  `fractions.Fraction` through dedicated handlers that return either primitive
  variants or structured records with explicit fields.
- Keep bool detection strict (`PyBool_Check`) to avoid treating custom objects
  as booleans.

**Tests:** Unit/property tests for overflow paths, `NaN` normalisation, complex
pairs, and decimal precision.

### WS4 – Rust Text, Binary, and Paths
**Scope:** Encode textual and binary payloads with truncation metadata.
- Preserve existing string handling but enforce preview limits and emit a
  truncation flag when slicing occurs.
- Encode `bytes`, `bytearray`, and `memoryview` as base64 payloads in
  `ValueRecord::Raw`, capturing original length and truncation metadata in a
  companion struct or fields.
- Support path-like objects via `os.fspath()` and register dedicated type ids
  (e.g., `pathlib.Path`, `os.PathLike`).

**Tests:** Verify ASCII/UTF-8 round-trips, binary truncation, and Windows vs
POSIX path serialisation.

### WS5 – Rust Collections & Recursion Management
**Scope:** Cover container types while preventing runaway traversal.
- Handle `set`, `frozenset`, `deque`, `range`, and general
  `collections.abc.Sequence`/`Mapping` subclasses. Represent set membership with
  unordered metadata; maintain insertion order for dicts by default.
- Emit `ValueRecord::Reference` when encountering previously seen object ids and
  mark truncated collections with `is_slice` or explicit preview counts.
- Stress-test mutually recursive and extremely deep structures to validate guard
  behaviour.

**Tests:** Unit tests for cycle detection, breadth limits, and ordering; fixture
comparisons for nested containers.

### WS6 – Rust Structured & Temporal Types
**Scope:** Capture object state for high-level constructs.
- Encode `dataclasses`, `attrs`, `typing.NamedTuple`, `enum.Enum`, and
  `types.SimpleNamespace` as `ValueRecord::Struct` with stable field ordering and
  qualified type names.
- Add handlers for `datetime`, `date`, `time`, `timedelta`, and `timezone`
  values, emitting ISO-8601 strings plus component fields to retain precision.
- Ensure attribute access avoids invoking arbitrary user code (e.g., skip
  descriptors that raise) and surface failures as structured errors.

**Tests:** Cover naive/aware datetimes, enum aliases, dataclass default
factories, and attrs slots.

### WS7 – Rust Fallback & Telemetry
**Scope:** Adopt `repr` fallback with diagnostics.
- Replace the `value.str()` fallback with `PyObject_Repr`, capturing exceptions
  and storing them as `ValueRecord::Error` with a deterministic type id.
- Annotate fallback records with handler metadata (e.g., which guard failed) so
  telemetry can track unsupported types.
- Update `runtime::value_capture` to attach truncation metadata and respect
  redaction/drop outcomes when the fallback path executes.

**Tests:** Regression tests for `repr` exceptions, gigantic repr strings,
truncation flags, and telemetry counters.

### WS8 – Hardening, Benchmarks, and Rollout
**Scope:** Validate performance, documentation, and communication.
- Benchmark encoding throughput/memory before and after the changes; capture
  results in `design-docs/expanded-value-encoding-implementation-plan.status.md`
  or a new metrics file.
- Update developer docs, onboarding guides, and contributor checklists to point
  at the registry and shared contract.
- Communicate the rollout to replay tooling teams, highlighting new metadata and
  truncation semantics.

**Exit criteria:** `just test` green, benchmarks captured, docs updated, ADR
flipped to **Accepted**.

## Milestones & Sequencing
1. **Milestone A – Contract & Infrastructure:** Complete WS1 and WS2 to establish
   the shared spec and registry without changing behaviour.
2. **Milestone B – Core Coverage:** Deliver WS3–WS5 (scalars, text/binary, and
   collections with recursion guards).
3. **Milestone C – Structured Types & Fallback:** Implement WS6 and WS7, enabling
   structured objects and `repr` fallback with telemetry.
4. **Milestone D – Hardening & Rollout:** Execute WS8, ensuring benchmarks,
   documentation updates, and telemetry checks.

## Verification Strategy
- Add targeted `cargo test` modules for registry helpers, handlers, and fallback
  paths; run Miri or sanitizers on recursion-guard suites when feasible.
- Extend `just test` to compare recorder output against the shared fixtures and
  add integration coverage for encoder edge cases.
- Include integration scripts that execute representative workloads (I/O heavy,
  recursion heavy, datetime heavy) and diff trace output against known-good
  snapshots.
- Gate merges on CI for Linux/macOS/Windows to catch locale and filesystem
  differences.

## Risks & Mitigations
- **Performance regressions:** Benchmark at each milestone; enforce depth/item
  budgets and avoid unnecessary allocations in tight loops.
- **Recursion or truncation bugs:** Guard with exhaustive unit tests plus
  property tests for nested containers; fall back to references rather than
  panicking.
- **Contract drift:** Shared fixtures and CI checks keep the encoder aligned with
  the documented specification.
- **Security concerns from repr/attribute access:** Restrict handlers to safe
  attribute reads, wrap failures, and ensure redaction policies run before any
  encoding work.

## Dependencies
- Python standard library modules used by handlers (`decimal`, `fractions`,
  `dataclasses`, `pathlib`, etc.) must be available in target environments.
- Additional Rust crates may be needed (e.g., `itertools`, `base64`); evaluate
  licensing and performance impact before adding.
- Testing dependencies (`pytest`, `proptest`, fixture loaders) should be wired
  into CI workflows (`just test`).
