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

#[cfg(any(test, feature = "integration-test"))]
mod fixtures {
    use super::encode_value;
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;
    use pyo3::types::PyModule;
    use runtime_tracing::{
        Line, NonStreamingTraceWriter, TraceLowLevelEvent, TraceWriter, TypeId, TypeRecord,
        TypeSpecificInfo, ValueRecord,
    };
    use serde_json::{self, json, Value};
    use std::ffi::{CStr, CString};
    #[cfg(test)]
    use std::fs;
    use std::path::Path;
    #[cfg(test)]
    use std::path::PathBuf;

    #[cfg(test)]
    use serde::Deserialize;

    #[cfg(test)]
    #[derive(Debug, Deserialize)]
    pub(crate) struct FixtureFile {
        pub(crate) cases: Vec<FixtureCase>,
    }

    #[cfg(test)]
    #[derive(Debug, Deserialize)]
    pub(crate) struct FixtureCase {
        pub(crate) name: String,
        pub(crate) code: String,
        pub(crate) expr: String,
        pub(crate) expected: Value,
        #[serde(default)]
        pub(crate) source: Option<String>,
    }

    #[cfg(test)]
    impl FixtureCase {
        pub(crate) fn context(&self) -> String {
            match &self.source {
                Some(src) => format!("{}::{}", src, self.name),
                None => self.name.clone(),
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn load_fixture_cases() -> Vec<FixtureCase> {
        let mut cases = Vec::new();
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/values");
        if !base.exists() {
            return cases;
        }
        let entries = fs::read_dir(&base)
            .unwrap_or_else(|err| panic!("failed to read fixture dir {}: {}", base.display(), err));
        for entry in entries {
            let entry = entry.expect("failed to read fixture directory entry");
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let contents = fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("failed to read {}: {}", path.display(), err));
            let mut file: FixtureFile = serde_json::from_str(&contents)
                .unwrap_or_else(|err| panic!("invalid JSON in {}: {}", path.display(), err));
            let source = path
                .file_name()
                .and_then(|name| name.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| path.display().to_string());
            for mut case in file.cases.drain(..) {
                case.source = Some(source.clone());
                cases.push(case);
            }
        }
        cases
    }

    #[cfg(test)]
    pub(crate) fn encode_case(py: Python<'_>, case: &FixtureCase) -> PyResult<Value> {
        encode_snippet(py, case.code.as_str(), case.expr.as_str())
    }

    pub(crate) fn encode_snippet(py: Python<'_>, code: &str, expr: &str) -> PyResult<Value> {
        let code_cstr = CString::new(code)
            .map_err(|err| PyErr::new::<PyValueError, _>(format!("invalid code: {err}")))?;
        let filename_cstr =
            CStr::from_bytes_with_nul(b"<fixture>\0").expect("fixture filename literal");
        let module_name_cstr =
            CStr::from_bytes_with_nul(b"<fixture_module>\0").expect("fixture module literal");
        let module =
            PyModule::from_code(py, code_cstr.as_c_str(), filename_cstr, module_name_cstr)?;
        let globals = module.dict();
        let expr_cstr = CString::new(expr)
            .map_err(|err| PyErr::new::<PyValueError, _>(format!("invalid expr: {err}")))?;
        let value = py.eval(expr_cstr.as_c_str(), Some(&globals), Some(&globals))?;

        let mut writer = NonStreamingTraceWriter::new("<fixture>", &[]);
        writer.start(Path::new("<fixture>"), Line(1));

        let record = encode_value(py, &mut writer, &value);
        let type_records = collect_type_records(&writer.events);
        Ok(canonicalize(&record, &type_records))
    }

    fn collect_type_records(events: &[TraceLowLevelEvent]) -> Vec<TypeRecord> {
        events
            .iter()
            .filter_map(|event| match event {
                TraceLowLevelEvent::Type(record) => Some(record.clone()),
                _ => None,
            })
            .collect()
    }

    fn type_name(types: &[TypeRecord], type_id: TypeId) -> String {
        types
            .get(type_id.0)
            .map(|record| record.lang_type.clone())
            .unwrap_or_else(|| format!("#{}", type_id.0))
    }

    fn canonicalize(value: &ValueRecord, types: &[TypeRecord]) -> Value {
        match value {
            ValueRecord::None { type_id } => {
                json!({"kind": "None", "type": type_name(types, *type_id)})
            }
            ValueRecord::Bool { b, type_id } => {
                json!({"kind": "Bool", "type": type_name(types, *type_id), "b": b})
            }
            ValueRecord::Int { i, type_id } => {
                json!({"kind": "Int", "type": type_name(types, *type_id), "i": i})
            }
            ValueRecord::Float { f, type_id } => {
                json!({"kind": "Float", "type": type_name(types, *type_id), "f": f})
            }
            ValueRecord::String { text, type_id } => json!({
                "kind": "String",
                "type": type_name(types, *type_id),
                "text": text,
            }),
            ValueRecord::Raw { r, type_id } => json!({
                "kind": "Raw",
                "type": type_name(types, *type_id),
                "r": r,
            }),
            ValueRecord::Error { msg, type_id } => json!({
                "kind": "Error",
                "type": type_name(types, *type_id),
                "msg": msg,
            }),
            ValueRecord::Sequence {
                elements,
                is_slice,
                type_id,
            } => json!({
                "kind": "Sequence",
                "type": type_name(types, *type_id),
                "is_slice": is_slice,
                "elements": elements.iter().map(|elem| canonicalize(elem, types)).collect::<Vec<_>>(),
            }),
            ValueRecord::Tuple { elements, type_id } => json!({
                "kind": "Tuple",
                "type": type_name(types, *type_id),
                "elements": elements.iter().map(|elem| canonicalize(elem, types)).collect::<Vec<_>>(),
            }),
            ValueRecord::Struct {
                field_values,
                type_id,
            } => {
                let fields = types
                    .get(type_id.0)
                    .and_then(|record| match &record.specific_info {
                        TypeSpecificInfo::Struct { fields } => Some(fields),
                        _ => None,
                    });
                let values = field_values
                    .iter()
                    .enumerate()
                    .map(|(idx, val)| {
                        let mut entry = serde_json::Map::new();
                        if let Some(fields) = fields {
                            if let Some(field) = fields.get(idx) {
                                entry.insert("name".to_string(), Value::String(field.name.clone()));
                            }
                        }
                        entry.insert("value".to_string(), canonicalize(val, types));
                        Value::Object(entry)
                    })
                    .collect::<Vec<_>>();
                json!({
                    "kind": "Struct",
                    "type": type_name(types, *type_id),
                    "fields": values,
                })
            }
            ValueRecord::Variant {
                discriminator,
                contents,
                type_id,
            } => json!({
                "kind": "Variant",
                "type": type_name(types, *type_id),
                "discriminator": discriminator,
                "contents": canonicalize(contents, types),
            }),
            ValueRecord::Reference {
                dereferenced,
                address,
                mutable,
                type_id,
            } => json!({
                "kind": "Reference",
                "type": type_name(types, *type_id),
                "address": address,
                "mutable": mutable,
                "dereferenced": canonicalize(dereferenced, types),
            }),
            ValueRecord::Cell { place } => json!({
                "kind": "Cell",
                "place": place.0,
            }),
            ValueRecord::BigInt {
                b,
                negative,
                type_id,
            } => json!({
                "kind": "BigInt",
                "type": type_name(types, *type_id),
                "negative": negative,
                "digits": b.iter().map(|byte| Value::from(*byte)).collect::<Vec<_>>(),
            }),
        }
    }
}

#[cfg(feature = "integration-test")]
use fixtures::encode_snippet;

#[cfg(feature = "integration-test")]
use pyo3::exceptions::PyValueError;

#[cfg(feature = "integration-test")]
#[pyfunction(name = "encode_value_fixture")]
pub(crate) fn encode_value_fixture_for_tests(
    py: Python<'_>,
    code: &str,
    expr: &str,
) -> PyResult<String> {
    let value = encode_snippet(py, code, expr)?;
    serde_json::to_string(&value)
        .map_err(|err| PyValueError::new_err(format!("failed to serialise encoded value: {err}")))
}

#[cfg(test)]
mod tests {
    use super::fixtures::{encode_case, load_fixture_cases};
    use pyo3::prelude::*;

    #[test]
    fn value_encoding_fixtures_match_contract() {
        let cases = load_fixture_cases();
        assert!(
            !cases.is_empty(),
            "expected at least one fixture case; add tests/data/values/*.json"
        );

        Python::with_gil(|py| {
            for case in cases {
                let context = case.context();
                let actual = encode_case(py, &case)
                    .unwrap_or_else(|err| panic!("encoding failed for {context}: {err}"));
                if actual != case.expected {
                    panic!(
                        "value mismatch for {context}\nexpected: {}\nactual: {}",
                        serde_json::to_string_pretty(&case.expected).unwrap(),
                        serde_json::to_string_pretty(&actual).unwrap()
                    );
                }
            }
        });
    }
}
