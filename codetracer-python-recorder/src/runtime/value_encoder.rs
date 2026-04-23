//! Encode Python values for trace recording.
//!
//! Two encoding paths are provided:
//!
//! 1. **Tree-based** ([`encode_value`]): Builds a `ValueRecord` tree from
//!    Python objects. This is the legacy path — it allocates `Vec`, `String`,
//!    and `Box` for every nested value.
//!
//! 2. **Streaming** ([`encode_value_streaming`]): Walks the Python object graph
//!    and calls `StreamingValueEncoder` C FFI methods directly, producing CBOR
//!    bytes without intermediate allocations. Cyclic references are detected
//!    using Python's `id()` (object identity). This is the M58 path.

use std::collections::HashSet;

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList, PyTuple};
use codetracer_trace_types::{TypeKind, ValueRecord, NONE_VALUE};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::StreamingValueEncoder;

/// Maximum recursion depth for streaming encoding. Protects against
/// pathological nesting that would overflow the encoder's compound stack
/// (which supports 32 levels) or the Rust call stack.
const MAX_STREAMING_DEPTH: usize = 30;

/// Convert Python values into `ValueRecord` instances understood by
/// `runtime_tracing`. Nested containers are encoded recursively and reuse the
/// tracer's type registry to ensure deterministic identifiers.
///
/// This is the legacy tree-based path. Prefer [`encode_value_streaming`] for
/// new code — it avoids intermediate `ValueRecord` allocations entirely.
pub fn encode_value<'py>(
    py: Python<'py>,
    writer: &mut dyn TraceWriter,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    if value.is_none() {
        return NONE_VALUE;
    }

    if let Ok(b) = value.extract::<bool>() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Bool, "Bool");
        return ValueRecord::Bool { b, type_id: ty };
    }

    if let Ok(i) = value.extract::<i64>() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Int, "Int");
        return ValueRecord::Int { i, type_id: ty };
    }

    if let Ok(s) = value.extract::<String>() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::String, "String");
        return ValueRecord::String {
            text: s,
            type_id: ty,
        };
    }

    if let Ok(tuple) = value.downcast::<PyTuple>() {
        let mut elements = Vec::with_capacity(tuple.len());
        for item in tuple.iter() {
            elements.push(encode_value(py, writer, &item));
        }
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Tuple, "Tuple");
        return ValueRecord::Tuple {
            elements,
            type_id: ty,
        };
    }

    if let Ok(list) = value.downcast::<PyList>() {
        let mut elements = Vec::with_capacity(list.len());
        for item in list.iter() {
            elements.push(encode_value(py, writer, &item));
        }
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Seq, "List");
        return ValueRecord::Sequence {
            elements,
            is_slice: false,
            type_id: ty,
        };
    }

    if let Ok(dict) = value.downcast::<PyDict>() {
        let seq_ty = TraceWriter::ensure_type_id(writer, TypeKind::Seq, "Dict");
        let tuple_ty = TraceWriter::ensure_type_id(writer, TypeKind::Tuple, "Tuple");
        let str_ty = TraceWriter::ensure_type_id(writer, TypeKind::String, "String");
        let mut elements = Vec::with_capacity(dict.len());
        for pair in dict.items().iter() {
            if let Ok(pair_tuple) = pair.downcast::<PyTuple>() {
                if pair_tuple.len() == 2 {
                    let key = pair_tuple.get_item(0).unwrap();
                    let value = pair_tuple.get_item(1).unwrap();
                    let key_record = if let Ok(text) = key.extract::<String>() {
                        ValueRecord::String {
                            text,
                            type_id: str_ty,
                        }
                    } else {
                        encode_value(py, writer, &key)
                    };
                    let value_record = encode_value(py, writer, &value);
                    let pair_record = ValueRecord::Tuple {
                        elements: vec![key_record, value_record],
                        type_id: tuple_ty,
                    };
                    elements.push(pair_record);
                }
            }
        }
        return ValueRecord::Sequence {
            elements,
            is_slice: false,
            type_id: seq_ty,
        };
    }

    let ty = TraceWriter::ensure_type_id(writer, TypeKind::Raw, "Object");
    match value.str() {
        Ok(text) => ValueRecord::Raw {
            r: text.to_string_lossy().into_owned(),
            type_id: ty,
        },
        Err(_) => ValueRecord::Error {
            msg: "<unrepr>".to_string(),
            type_id: ty,
        },
    }
}

// ---------------------------------------------------------------------------
// Streaming encoder (M58) — encodes Python values directly to CBOR bytes
// ---------------------------------------------------------------------------

/// Walk a Python object and encode it directly to CBOR using the streaming
/// value encoder. Returns the encoded CBOR bytes as a `Vec<u8>`.
///
/// The `seen` set tracks Python object identities (`id()`) to detect cyclic
/// references. When a cycle is detected, the encoder emits a
/// `ValueRecord::Error` sentinel ("<cycle>") instead of recursing infinitely.
///
/// The `seen` set must be reset between top-level value encodings (per-step),
/// not globally. The caller is responsible for providing a fresh `HashSet`
/// for each value being encoded.
///
/// # Type IDs
///
/// The streaming encoder needs type IDs from the `TraceWriter`'s type registry.
/// Since the streaming encoder and writer are separate objects, type IDs must
/// be obtained from the writer before encoding. This function takes a mutable
/// reference to the writer to call `ensure_type_id`.
pub fn encode_value_streaming<'py>(
    py: Python<'py>,
    writer: &mut dyn TraceWriter,
    encoder: &mut StreamingValueEncoder,
    value: &Bound<'py, PyAny>,
) -> Vec<u8> {
    let mut seen = HashSet::new();
    encoder.reset();
    encode_streaming_recursive(py, writer, encoder, value, &mut seen, 0);
    encoder.get_bytes_copy()
}

/// Recursive streaming encoder. Walks the Python object graph and calls
/// streaming C FFI methods directly, producing CBOR bytes without building
/// intermediate `ValueRecord` trees.
fn encode_streaming_recursive<'py>(
    py: Python<'py>,
    writer: &mut dyn TraceWriter,
    encoder: &mut StreamingValueEncoder,
    value: &Bound<'py, PyAny>,
    seen: &mut HashSet<isize>,
    depth: usize,
) {
    // Depth guard: prevent stack overflow from pathological nesting.
    if depth >= MAX_STREAMING_DEPTH {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Raw, "Object");
        encoder.write_raw("<depth limit>", ty);
        return;
    }

    if value.is_none() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Raw, "NoneType");
        encoder.write_none(ty);
        return;
    }

    // For compound types, check for cycles using Python's id().
    // Leaf types (int, float, bool, str) cannot be cyclic, so we skip
    // the id check for them — it would be wasted work.

    if let Ok(b) = value.extract::<bool>() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Bool, "Bool");
        encoder.write_bool(b, ty);
        return;
    }

    if let Ok(i) = value.extract::<i64>() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Int, "Int");
        encoder.write_int(i, ty);
        return;
    }

    // Try float extraction. Python floats are always f64.
    if let Ok(f) = value.extract::<f64>() {
        // Only accept if the value is actually a Python float, not an int
        // that was coerced. Check the Python type name to distinguish.
        let is_float = value
            .get_type()
            .name()
            .map(|n| n == "float")
            .unwrap_or(false);
        if is_float {
            let ty = TraceWriter::ensure_type_id(writer, TypeKind::Float, "Float");
            encoder.write_float(f, ty);
            return;
        }
    }

    if let Ok(s) = value.extract::<String>() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::String, "String");
        encoder.write_string(&s, ty);
        return;
    }

    // --- Compound types: cycle detection ---

    // Get the Python object id for cycle detection on compound types.
    let obj_id = value.as_ptr() as isize;
    if !seen.insert(obj_id) {
        // Cycle detected — this object was already seen in the current walk.
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Raw, "Cycle");
        encoder.write_error("<cycle>", ty);
        return;
    }

    if let Ok(tuple) = value.downcast::<PyTuple>() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Tuple, "Tuple");
        encoder.begin_tuple(ty, tuple.len());
        for item in tuple.iter() {
            encode_streaming_recursive(py, writer, encoder, &item, seen, depth + 1);
        }
        encoder.end_compound();
        seen.remove(&obj_id);
        return;
    }

    if let Ok(list) = value.downcast::<PyList>() {
        let ty = TraceWriter::ensure_type_id(writer, TypeKind::Seq, "List");
        encoder.begin_sequence(ty, list.len());
        for item in list.iter() {
            encode_streaming_recursive(py, writer, encoder, &item, seen, depth + 1);
        }
        encoder.end_compound();
        seen.remove(&obj_id);
        return;
    }

    if let Ok(dict) = value.downcast::<PyDict>() {
        let seq_ty = TraceWriter::ensure_type_id(writer, TypeKind::Seq, "Dict");
        let tuple_ty = TraceWriter::ensure_type_id(writer, TypeKind::Tuple, "Tuple");
        let str_ty = TraceWriter::ensure_type_id(writer, TypeKind::String, "String");

        // Encode dict as a sequence of (key, value) tuples, matching the
        // tree-based encoder's representation for backward compatibility.
        encoder.begin_sequence(seq_ty, dict.len());
        for pair in dict.items().iter() {
            if let Ok(pair_tuple) = pair.downcast::<PyTuple>() {
                if pair_tuple.len() == 2 {
                    let key = pair_tuple.get_item(0).unwrap();
                    let val = pair_tuple.get_item(1).unwrap();

                    encoder.begin_tuple(tuple_ty, 2);
                    // Optimize string keys: extract directly without recursion.
                    if let Ok(text) = key.extract::<String>() {
                        encoder.write_string(&text, str_ty);
                    } else {
                        encode_streaming_recursive(py, writer, encoder, &key, seen, depth + 1);
                    }
                    encode_streaming_recursive(py, writer, encoder, &val, seen, depth + 1);
                    encoder.end_compound();
                }
            }
        }
        encoder.end_compound();
        seen.remove(&obj_id);
        return;
    }

    // Fallback: use Python's str() representation as a Raw value.
    let ty = TraceWriter::ensure_type_id(writer, TypeKind::Raw, "Object");
    match value.str() {
        Ok(text) => encoder.write_raw(&text.to_string_lossy(), ty),
        Err(_) => encoder.write_error("<unrepr>", ty),
    }
    seen.remove(&obj_id);
}
