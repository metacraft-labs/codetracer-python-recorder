//! Encode Python values into `runtime_tracing` records.

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList, PyTuple};
use runtime_tracing::{NonStreamingTraceWriter, TraceWriter, TypeKind, ValueRecord, NONE_VALUE};

/// Convert Python values into `ValueRecord` instances understood by
/// `runtime_tracing`. Nested containers are encoded recursively and reuse the
/// tracer's type registry to ensure deterministic identifiers.
pub fn encode_value<'py>(
    py: Python<'py>,
    writer: &mut NonStreamingTraceWriter,
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
