# Runtime Recorder Value Encoding Contract

This document defines the canonical representation of Python values produced by
`codetracer-python-recorder`. It complements ADR 0018 and keeps the encoding
contract decoupled from implementation details inside
`src/runtime/value_encoder.rs`. Contributors should treat this document as the
source of truth when implementing or reviewing value-handling changes.

## Scope & Invariants

- The contract applies to the Rust recorder only; the legacy pure-Python tracer
  is deprecated.
- Encoding always happens after `ValuePolicy` redaction/drop decisions. Encoders
  must **never** bypass policy outcomes.
- Every value written to the trace resolves to one of the `ValueRecord`
  variants. Unsupported objects fall back to `ValueRecord::Raw` (repr) or
  `ValueRecord::Error` (repr failure).
- Type identifiers are registered through
  `TraceWriter::ensure_type_id(writer, kind, name)` and remain stable across
  runs. Use fully-qualified names (e.g., `builtins.int`, `decimal.Decimal`,
  `pathlib.Path`).
- Traversal is guarded by:
  - A seen-map keyed by object identity to prevent infinite recursion.
  - Depth and breadth budgets on nested containers and large payloads.
  - `ValueRecord::Reference` emitted whenever an object reappears after its
    first structured encoding.
- Truncation is explicit. Partial data must set `is_slice: true` (for sequences)
  or include metadata fields such as `preview_bytes` and `total_bytes`.

## ValueRecord Variants

| Variant                | Usage                                                                                   |
|------------------------|-----------------------------------------------------------------------------------------|
| `None { type_id }`     | `None` sentinel.                                                                        |
| `Bool { b, type_id }`  | Boolean values, no implicit coercion from custom types.                                 |
| `Int { i, type_id }`   | Signed 64-bit integers.                                                                 |
| `Float { f, type_id }` | IEEE-754 doubles (future extension; use Raw until implemented).                         |
| `String { text, type_id }` | UTF-8 text.                                                                        |
| `Raw { r, type_id }`   | Repr fallback or encoded binary payloads.                                               |
| `Error { msg, type_id }` | Failure to encode or repr.                                                          |
| `Tuple { elements, type_id }` | Fixed-length positional records.                                                |
| `Sequence { elements, is_slice, type_id }` | Variable-length collections (lists, sets, ranges, etc.).          |
| `Struct { field_names, field_values, type_id }` | Named records (dataclasses, enums, etc.).                     |
| `Reference { target_id, type_id }` | Identifies a previously emitted object.                                     |

> _Note:_ Some variants (e.g., `Float`, `Struct`, `Reference`) are not yet used
> in the current implementation. They are included here to document the desired
> end state and should be adopted during WS3–WS6.

## Type Metadata

1. Prefer module-qualified names:
   - `builtins.int`, `builtins.list`, `builtins.set`
   - `decimal.Decimal`, `fractions.Fraction`
   - `datetime.datetime`, `pathlib.Path`
2. Synthetic helpers follow `codetracer.*` (e.g., `codetracer.range`,
   `codetracer.bytes-preview`).
3. Collection instances share the same type id as their class. Truncation
   metadata is recorded alongside the value, not in the type registry.

## Scalars

- **None** — Emit `ValueRecord::None` using the existing `NONE_VALUE` constant.
- **Booleans** — Detect via `PyBool_Check`. Encode as
  `ValueRecord::Bool { b, type_id: ensure(TypeKind::Bool, "builtins.bool") }`.
- **Integers** — Attempt to downcast to `i64`. When successful, emit
  `ValueRecord::Int`. Otherwise encode as:
  ```text
  ValueRecord::Struct {
      type_id: ensure(TypeKind::Struct, "builtins.int"),
      field_names: ["sign", "digits"],
      field_values: [
          ValueRecord::Int { i: -1 | 1 },
          ValueRecord::String { text: "<decimal digits>" },
      ],
  }
  ```
  `sign` is -1 or 1. `digits` contains the absolute value in base-10.
- **Floats** — Emit `ValueRecord::Float`. Preserve `NaN`/`±Inf` verbatim; replay
  tooling should handle special values.
- **Complex** — Encode as a tuple of two floats with type name `builtins.complex`.
- **Decimals/Fractions** — Use `ValueRecord::Struct` with components:
  - `decimal.Decimal`: fields `["sign", "exponent", "digits"]`.
  - `fractions.Fraction`: fields `["numerator", "denominator"]` as integers.

## Text & Binary

- **Strings** — Emit full text when length ≤ `STRING_PREVIEW_LIMIT`. When longer,
  truncate to the first `STRING_PREVIEW_LIMIT` characters, set `is_slice: true`,
  and add a synthetic struct alongside the string:
  ```text
  ValueRecord::Struct {
      type_id: ensure(TypeKind::Struct, "codetracer.string-preview"),
      field_names: ["preview", "total_length", "truncated"],
      field_values: [
          ValueRecord::String { text: "<preview>" },
          ValueRecord::Int { i: original_len },
          ValueRecord::Bool { b: true },
      ],
  }
  ```
- **Bytes/Bytearray/MemoryView** — Base64-encode the first
  `BINARY_PREVIEW_BYTES` bytes and store inside `ValueRecord::Raw { r }` with
  type name `builtins.bytes` (or `builtins.bytearray`). Emit metadata struct:
  `codetracer.bytes-preview` with `["preview_b64", "total_bytes", "truncated"]`.
- **Path-like objects** — Call `os.fspath()`; encode result as
  `ValueRecord::String` with type name `pathlib.Path` or the implementing class.

## Collections & Iterables

- **Tuples** — Encode as `ValueRecord::Tuple`. Elements preserve order.
- **Lists/Sequences** — Encode as `ValueRecord::Sequence` with `is_slice` flag
  when truncated. Preserve insertion order.
- **Sets/Frozensets** — Treat as sequences but add a companion struct with
  `codetracer.set-metadata`, including `["unordered", "preview_count", "total_count"]`.
  Avoid sorting by default; rely on insertion order where provided by CPython.
- **Deque/Range/Other Iterables** — Materialise up to `ITERABLE_PREVIEW_COUNT`
  items. Mark `is_slice` when truncated and record the exhaustion metadata.
- **References** — When encountering an object already in the seen-map, emit
  `ValueRecord::Reference { target_id, type_id }`. `target_id` is the numeric id
  assigned during the first emission.

## Mappings

- **Dicts/Mapping subclasses** — Emit as `ValueRecord::Sequence` of key/value
  tuples. The outer sequence has type `builtins.dict`, inner tuple type
  `codetracer.dict-entry`. Preserve iteration order. Referencing keys:
  - Direct string keys use `ValueRecord::String`.
  - Non-string keys are encoded recursively.
- **OrderedDict / default dict** — Reuse `builtins.dict` type id but attach
  metadata struct describing specialised behaviour (e.g.,
  `codetracer.collections.defaultdict` with fields `["default_factory"]`).

## Structured Objects

- **Dataclasses/attrs/NamedTuple** — Encode as `ValueRecord::Struct` with field
  names derived from the class definition. Respect `init=False`/`repr=False`
  flags by omitting suppressed fields.
- **Enums** — Emit structs with fields `["name", "value"]`.
- **SimpleNamespace / objects with `__dict__`** — Enumerate attributes,
  excluding private names (`__*__`) by default. Order fields lexicographically.
- **Slots-based objects** — Use `dir()`/`__slots__` metadata to gather fields,
  handling `AttributeError` gracefully.

## Temporal Values

- `datetime.datetime`, `datetime.date`, `datetime.time`, `datetime.timedelta`,
  and `datetime.tzinfo` encode as structs with ISO-8601 string previews plus
  component fields (year, month, offset, etc.). Example:
  ```text
  ValueRecord::Struct {
      type_id: ensure(TypeKind::Struct, "datetime.datetime"),
      field_names: ["isoformat", "timestamp", "tzinfo"],
      field_values: [
          ValueRecord::String { text: dt.isoformat() },
          ValueRecord::Float { f: dt.timestamp() },
          <tzinfo struct or None>,
      ],
  }
  ```
- Truncate microseconds if policy requires but always report the original value.

## Errors & Fallback

- **repr fallback** — For unsupported objects, call `PyObject_Repr` and emit:
  `ValueRecord::Raw { r: repr_string, type_id: ensure(TypeKind::Raw, "<module>.<type>") }`.
- **repr failure** — When `repr` raises, emit `ValueRecord::Error` with:
  - `msg`: `"<unrepr>: <exception repr>"`
  - `type_id`: same as the attempted fallback.
- **Encoding failure** — Catch unexpected errors, log with `ScopedMuteIoCapture`,
  and emit `ValueRecord::Error` plus a telemetry counter.

## Fixtures

Golden fixtures for this contract live under
`codetracer-python-recorder/tests/data/values/`. Each fixture stores the Python
code snippet, the captured value, and the expected `ValueRecord` JSON structure.
Both unit tests and integration tests must load these fixtures to prevent
regressions.

## Status Tracking

- WS1 (Contract & Fixtures) owns this document. Any changes require updating
  the implementation-plan status file and notifying recorder maintainers.
- Downstream tooling teams consume this doc to stay informed about metadata
  fields and truncation semantics.
