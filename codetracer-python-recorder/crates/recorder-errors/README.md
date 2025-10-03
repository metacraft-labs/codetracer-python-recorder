# recorder-errors

Shared error handling primitives for the CodeTracer recorders. This crate
provides the `RecorderError` type, error classification enums, ergonomic macros,
and opt-in serde support so higher-level crates can transport structured
failures across process and FFI boundaries.
