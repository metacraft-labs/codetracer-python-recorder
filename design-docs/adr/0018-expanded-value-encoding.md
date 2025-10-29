# ADR 0018: Expand Value Encoding Coverage with Repr Fallback

- **Status:** Proposed
- **Date:** 2025-10-07
- **Deciders:** Runtime recorder maintainers
- **Consulted:** Observability WG, Replay tooling crew
- **Informed:** DX crew, Release crew

## Context

Following ADR 0011 the Rust-backed recorder (`codetracer-python-recorder`) now
routes value capture through dedicated collaborators. `runtime::value_capture`
enforces policy decisions, emits `<redacted>` and `<dropped>` sentinels, and
delegates to `runtime::value_encoder::encode_value`. That encoder still
recognises only `None`, `bool`, 64-bit `int`, UTF-8 `str`, tuples, lists, and
dicts; all other values fall back to `PyAny::str()`. We neither detect recursion
nor preserve high-precision numerics, and `repr` failures surface only as
`<unrepr>`.

The legacy pure-Python recorder has been deprecated and is no longer maintained.
Future work focuses exclusively on the Rust implementation.

Downstream tooling expects richer structure (floats, decimals, bytes, dataclasses,
path-like objects, temporal types, etc.) to reason about traces deterministically.
The refactor improved modularity but left encoding unchanged, so the recorder still
emits lossy snapshots that replay consumers must reinterpret by hand.

## Decision

1. **Define a shared encoding contract.** Refresh `design-docs/encode-values.md`
   to describe canonical value shapes, type identifiers, recursion limits, and
   truncation metadata. The contract applies to the recorder and is backed by
   shared fixtures.

2. **Introduce structured encoders in the Rust recorder.** Replace the
   monolithic `encode_value` cascade with a registry of guard/handler pairs that
   can be extended without editing hot code paths. Handlers operate on
   `NonStreamingTraceWriter`, honour `ValuePolicy` redactions, and register
   deterministic type identifiers via `TraceWriter::ensure_type_id`. The registry
   enforces recursion and breadth guards, surfaces self-references as
   `ValueRecord::Reference`, and emits truncation metadata alongside partial
   results.

3. **Adopt `repr` as the universal fallback.** When no handler matches, call
   `repr(obj)` and store the result as `ValueRecord::Raw` with an explicit type
   name (e.g., `builtins.object`). If `repr` fails, emit `ValueRecord::Error`
   (`<unrepr>` plus the exception string) so downstream consumers can detect the
   escape hatch. The fallback honours redaction/drop decisions and recursion
   guards.

4. **Expand structured coverage across standard categories.** Iteratively add
   handlers for scalars (arbitrary `int`, `float`, `complex`, `decimal.Decimal`,
   `fractions.Fraction`), text/binary (`bytes`, `bytearray`, `memoryview`,
   path-like objects), collections (`set`, `frozenset`, `deque`, ranges and other
   `collections.abc` implementations), records (`dataclasses`, `attrs`,
   `NamedTuple`, `Enum`, `SimpleNamespace`), and temporal values (`datetime`,
   `date`, `time`, `timedelta`, `timezone`). Each handler advertises precise
   truncation and metadata so replay tools can render meaningful previews.

## Consequences

- Recorder snapshots gain structured coverage for CPython’s built-in and
  standard-library types, improving reproducibility for replay tooling and
  analytics.
- A registry-based encoder and shared contract make incremental contributions
  straightforward, while policy checks and type registration remain centralised.
- Complexity rises: recursion guards, truncation metadata, and richer fixtures
  require careful benchmarking to stay within tracing latency budgets.

## Alternatives

- **Status quo:** continue relying on `str(obj)` in the recorder. This keeps
  the implementation simple but leaves traces lossy and undermines downstream
  tooling.
- **Opt-in plugins only:** expose hooks for external encoders without shipping
  defaults. This delays improved coverage, complicates support, and fragments
  the trace format between environments.
- **Pickle everything:** serialising arbitrary objects via `pickle` would capture
  more data but introduces security risks, large payloads, and Python-version
  incompatibilities.

## Rollout

1. Refresh the shared encoding contract and publish shared fixtures.
2. Land the Rust registry scaffolding and migrate existing cases to it without
   changing behaviour.
3. Implement structured handlers in Rust, accompanied by unit tests and property
   checks for recursion and truncation.
4. Flip the fallback to `repr`, add telemetry for truncation/error escapes, and
   update documentation before promoting the ADR to **Accepted**.

## Open Questions

- How should we negotiate recursion limits and preview lengths with user
  configuration now that `ValuePolicy` can already drop or redact values?
- What metadata do replay consumers need to distinguish truncated binary/text
  payloads from complete ones without bloating trace size?
- What is the right strategy for sharing encoder fixtures with downstream tools
  without introducing heavyweight build dependencies?
