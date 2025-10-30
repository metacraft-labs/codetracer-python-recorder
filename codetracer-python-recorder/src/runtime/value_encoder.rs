//! Encode Python values into `runtime_tracing` records via a registry of handlers.
//!
//! The encoder coordinates a registry of type-specific handlers. Each handler owns
//! a guard predicate and encoding callback, while [`ValueEncoderContext`] stores
//! recursion guards, traversal budgets, and shared writer state. This keeps the
//! encoding surface extensible for future workspaces (WS3–WS6) without coupling
//! handlers to policy or tracing concerns.

use once_cell::sync::Lazy;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyList, PyTuple};
use runtime_tracing::{
    NonStreamingTraceWriter, TraceWriter, TypeId, TypeKind, ValueRecord, NONE_VALUE,
};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;

const DEFAULT_MAX_DEPTH: usize = 32;
const DEFAULT_MAX_SEQUENCE_ITEMS: usize = 64;

#[derive(Debug, Clone, Copy)]
pub struct EncoderLimits {
    pub max_depth: usize,
    pub max_items: usize,
}

impl Default for EncoderLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_items: DEFAULT_MAX_SEQUENCE_ITEMS,
        }
    }
}

struct ValueHandler {
    guard: fn(&Bound<'_, PyAny>) -> bool,
    encode: for<'py> fn(&mut ValueEncoderContext<'py, '_>, &Bound<'py, PyAny>) -> ValueRecord,
}

impl ValueHandler {
    const fn new(
        guard: fn(&Bound<'_, PyAny>) -> bool,
        encode: for<'py> fn(&mut ValueEncoderContext<'py, '_>, &Bound<'py, PyAny>) -> ValueRecord,
    ) -> Self {
        Self { guard, encode }
    }
}

static HANDLERS: Lazy<Vec<ValueHandler>> = Lazy::new(|| {
    vec![
        ValueHandler::new(guard_none, encode_none),
        ValueHandler::new(guard_bool, encode_bool),
        ValueHandler::new(guard_int, encode_int),
        ValueHandler::new(guard_string, encode_string),
        ValueHandler::new(guard_tuple, encode_tuple),
        ValueHandler::new(guard_list, encode_list),
        ValueHandler::new(guard_dict, encode_dict),
    ]
});

pub(crate) struct ValueEncoderContext<'py, 'writer> {
    _py: PhantomData<Python<'py>>,
    writer: &'writer mut NonStreamingTraceWriter,
    limits: EncoderLimits,
    depth: usize,
    in_progress: HashSet<usize>,
    memo: HashMap<usize, ValueRecord>,
}

impl<'py, 'writer> ValueEncoderContext<'py, 'writer> {
    pub(crate) fn new(py: Python<'py>, writer: &'writer mut NonStreamingTraceWriter) -> Self {
        Self::with_limits(py, writer, EncoderLimits::default())
    }

    pub(crate) fn with_limits(
        _py: Python<'py>,
        writer: &'writer mut NonStreamingTraceWriter,
        limits: EncoderLimits,
    ) -> Self {
        Self {
            _py: PhantomData,
            writer,
            limits,
            depth: 0,
            in_progress: HashSet::new(),
            memo: HashMap::new(),
        }
    }

    pub(crate) fn encode_root(&mut self, value: &Bound<'py, PyAny>) -> ValueRecord {
        self.dispatch(value)
    }

    fn encode_nested(&mut self, value: &Bound<'py, PyAny>) -> ValueRecord {
        if self.depth >= self.limits.max_depth {
            return self.encode_repr(value);
        }
        self.depth += 1;
        let record = self.dispatch(value);
        self.depth -= 1;
        record
    }

    fn dispatch(&mut self, value: &Bound<'py, PyAny>) -> ValueRecord {
        let ptr = value.as_ptr() as usize;
        if let Some(existing) = self.memo.get(&ptr).cloned() {
            return self.make_reference(ptr, value, existing);
        }

        let track = self.should_track(value);
        if track && !self.in_progress.insert(ptr) {
            return self.encode_repr(value);
        }

        let record = HANDLERS
            .iter()
            .find(|handler| (handler.guard)(value))
            .map(|handler| (handler.encode)(self, value))
            .unwrap_or_else(|| self.encode_repr(value));

        if track {
            self.in_progress.remove(&ptr);
            self.memo.insert(ptr, record.clone());
        }

        record
    }

    fn ensure_type(&mut self, kind: TypeKind, name: &'static str) -> TypeId {
        TraceWriter::ensure_type_id(self.writer, kind, name)
    }

    fn encode_repr(&mut self, value: &Bound<'py, PyAny>) -> ValueRecord {
        let ty = self.ensure_type(TypeKind::Raw, "Object");
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

    fn should_track(&self, value: &Bound<'_, PyAny>) -> bool {
        value.downcast::<PyList>().is_ok()
            || value.downcast::<PyDict>().is_ok()
            || value.downcast::<PyTuple>().is_ok()
    }

    fn is_mutable(&self, value: &Bound<'_, PyAny>) -> bool {
        value.downcast::<PyList>().is_ok() || value.downcast::<PyDict>().is_ok()
    }

    fn make_reference(
        &mut self,
        ptr: usize,
        value: &Bound<'_, PyAny>,
        canonical: ValueRecord,
    ) -> ValueRecord {
        let type_id =
            record_type_id(&canonical).unwrap_or_else(|| self.ensure_type(TypeKind::Raw, "Object"));
        ValueRecord::Reference {
            dereferenced: Box::new(canonical),
            address: ptr as u64,
            mutable: self.is_mutable(value),
            type_id,
        }
    }
}

pub fn encode_value<'py>(
    py: Python<'py>,
    writer: &mut NonStreamingTraceWriter,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let mut context = ValueEncoderContext::new(py, writer);
    context.encode_root(value)
}

fn guard_none(value: &Bound<'_, PyAny>) -> bool {
    value.is_none()
}

fn encode_none(_ctx: &mut ValueEncoderContext<'_, '_>, _value: &Bound<'_, PyAny>) -> ValueRecord {
    NONE_VALUE
}

fn guard_bool(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyBool>().is_ok()
}

fn encode_bool(ctx: &mut ValueEncoderContext<'_, '_>, value: &Bound<'_, PyAny>) -> ValueRecord {
    let ty = ctx.ensure_type(TypeKind::Bool, "Bool");
    let b = value.extract::<bool>().unwrap_or(false);
    ValueRecord::Bool { b, type_id: ty }
}

fn guard_int(value: &Bound<'_, PyAny>) -> bool {
    value.extract::<i64>().is_ok()
}

fn encode_int(ctx: &mut ValueEncoderContext<'_, '_>, value: &Bound<'_, PyAny>) -> ValueRecord {
    let ty = ctx.ensure_type(TypeKind::Int, "Int");
    let i = value.extract::<i64>().unwrap_or_default();
    ValueRecord::Int { i, type_id: ty }
}

fn guard_string(value: &Bound<'_, PyAny>) -> bool {
    value.extract::<String>().is_ok()
}

fn encode_string(ctx: &mut ValueEncoderContext<'_, '_>, value: &Bound<'_, PyAny>) -> ValueRecord {
    let ty = ctx.ensure_type(TypeKind::String, "String");
    let text = value.extract::<String>().unwrap_or_else(|_| String::new());
    ValueRecord::String { text, type_id: ty }
}

fn guard_tuple(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyTuple>().is_ok()
}

fn encode_tuple<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let tuple = value.downcast::<PyTuple>().expect("guard ensures tuple");
    let mut elements = Vec::with_capacity(tuple.len());
    for item in tuple.iter() {
        let record = ctx.encode_nested(&item);
        elements.push(record);
    }
    let ty = ctx.ensure_type(TypeKind::Tuple, "Tuple");
    ValueRecord::Tuple {
        elements,
        type_id: ty,
    }
}

fn guard_list(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyList>().is_ok()
}

fn encode_list<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let list = value.downcast::<PyList>().expect("guard ensures list");
    let len = list.len();
    let limit = len.min(ctx.limits.max_items);
    let mut elements = Vec::with_capacity(limit);
    for item in list.iter().take(limit) {
        let record = ctx.encode_nested(&item);
        elements.push(record);
    }
    let ty = ctx.ensure_type(TypeKind::Seq, "List");
    ValueRecord::Sequence {
        elements,
        is_slice: len > ctx.limits.max_items,
        type_id: ty,
    }
}

fn guard_dict(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyDict>().is_ok()
}

fn encode_dict<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let dict = value.downcast::<PyDict>().expect("guard ensures dict");
    let len = dict.len();
    let limit = len.min(ctx.limits.max_items);
    let tuple_ty = ctx.ensure_type(TypeKind::Tuple, "Tuple");
    let str_ty = ctx.ensure_type(TypeKind::String, "String");
    let mut elements = Vec::with_capacity(limit);
    for pair in dict.items().iter().take(limit) {
        if let Ok(pair_tuple) = pair.downcast::<PyTuple>() {
            if pair_tuple.len() != 2 {
                continue;
            }
            let key = pair_tuple.get_item(0).unwrap();
            let value = pair_tuple.get_item(1).unwrap();
            let key_record = if let Ok(text) = key.extract::<String>() {
                ValueRecord::String {
                    text,
                    type_id: str_ty,
                }
            } else {
                ctx.encode_nested(&key)
            };
            let value_record = ctx.encode_nested(&value);
            elements.push(ValueRecord::Tuple {
                elements: vec![key_record, value_record],
                type_id: tuple_ty,
            });
        }
    }
    let ty = ctx.ensure_type(TypeKind::Seq, "Dict");
    ValueRecord::Sequence {
        elements,
        is_slice: len > ctx.limits.max_items,
        type_id: ty,
    }
}

fn record_type_id(record: &ValueRecord) -> Option<TypeId> {
    match record {
        ValueRecord::Int { type_id, .. }
        | ValueRecord::Float { type_id, .. }
        | ValueRecord::Bool { type_id, .. }
        | ValueRecord::String { type_id, .. }
        | ValueRecord::Sequence { type_id, .. }
        | ValueRecord::Tuple { type_id, .. }
        | ValueRecord::Struct { type_id, .. }
        | ValueRecord::Variant { type_id, .. }
        | ValueRecord::Reference { type_id, .. }
        | ValueRecord::Raw { type_id, .. }
        | ValueRecord::Error { type_id, .. }
        | ValueRecord::None { type_id }
        | ValueRecord::BigInt { type_id, .. } => Some(*type_id),
        ValueRecord::Cell { .. } => None,
    }
}

#[cfg(any(test, feature = "integration-test"))]
mod fixtures {
    use super::ValueEncoderContext;
    use pyo3::exceptions::PyValueError;
    use pyo3::prelude::*;
    use pyo3::types::PyModule;
    use runtime_tracing::{
        Line, NonStreamingTraceWriter, TraceLowLevelEvent, TraceWriter, TypeId, TypeRecord,
        TypeSpecificInfo, ValueRecord,
    };
    #[cfg(test)]
    use serde::Deserialize;
    use serde_json::{self, json, Value};
    use std::ffi::{CStr, CString};
    #[cfg(test)]
    use std::fs;
    use std::path::Path;
    #[cfg(test)]
    use std::path::PathBuf;

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

        let mut context = ValueEncoderContext::new(py, &mut writer);
        let record = context.encode_root(&value);
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
    use super::{EncoderLimits, ValueEncoderContext, DEFAULT_MAX_DEPTH};
    use pyo3::prelude::*;
    use pyo3::types::{PyList, PyTuple};
    use runtime_tracing::{Line, NonStreamingTraceWriter, TraceWriter, ValueRecord};
    use std::ffi::CString;

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

    #[test]
    fn bool_handler_runs_before_int() {
        Python::with_gil(|py| {
            let expr = CString::new("True").expect("bool literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            let record = context.encode_root(&value);
            match record {
                ValueRecord::Bool { b, .. } => assert!(b),
                other => panic!("expected Bool record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn recursion_respects_depth_budget() {
        Python::with_gil(|py| {
            let nested = CString::new("[[[1]]]").expect("nested list literal");
            let value = py.eval(nested.as_c_str(), None, None).expect("nested list");
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let limits = EncoderLimits {
                max_depth: 1,
                max_items: usize::MAX,
            };
            let mut context = ValueEncoderContext::with_limits(py, &mut writer, limits);
            let record = context.encode_root(&value);
            let inner = match record {
                ValueRecord::Sequence { elements, .. } => elements.first().cloned(),
                other => panic!("expected outer sequence, found {:?}", other),
            }
            .expect("inner element");
            match inner {
                ValueRecord::Sequence { elements, .. } => match elements.first().cloned() {
                    Some(ValueRecord::Raw { .. }) | Some(ValueRecord::Error { .. }) => {}
                    other => panic!(
                        "expected raw fallback due to depth limit, found {:?}",
                        other
                    ),
                },
                other => panic!("expected nested sequence, found {:?}", other),
            }
        });
    }

    #[test]
    fn repeated_containers_emit_reference_records() {
        Python::with_gil(|py| {
            let inner_list = PyList::new(py, [1]).expect("inner list creation");
            let tuple = PyTuple::new(py, [inner_list.clone(), inner_list]).expect("tuple creation");
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            let tuple_any = tuple.into_any();
            let record = context.encode_root(&tuple_any);
            match record {
                ValueRecord::Tuple { elements, .. } => {
                    assert_eq!(elements.len(), 2);
                    assert!(matches!(
                        elements[1],
                        ValueRecord::Reference { mutable: true, .. }
                    ));
                }
                other => panic!("expected Tuple record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn list_truncation_sets_slice_flag() {
        Python::with_gil(|py| {
            let expr = CString::new("[0, 1, 2]").expect("list literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let limits = EncoderLimits {
                max_depth: DEFAULT_MAX_DEPTH,
                max_items: 1,
            };
            let mut context = ValueEncoderContext::with_limits(py, &mut writer, limits);
            let record = context.encode_root(&value);
            match record {
                ValueRecord::Sequence {
                    elements, is_slice, ..
                } => {
                    assert_eq!(elements.len(), 1);
                    assert!(is_slice, "expected slice flag after truncation");
                }
                other => panic!("expected Sequence record, saw {:?}", other),
            }
        });
    }
}
