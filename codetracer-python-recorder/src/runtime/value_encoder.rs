//! Encode Python values into `runtime_tracing` records via a registry of handlers.
//!
//! The encoder coordinates a registry of type-specific handlers. Each handler owns
//! a guard predicate and encoding callback, while [`ValueEncoderContext`] stores
//! recursion guards, traversal budgets, and shared writer state. This keeps the
//! encoding surface extensible for future workspaces (WS3–WS6) without coupling
//! handlers to policy or tracing concerns.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use once_cell::sync::Lazy;
use pyo3::prelude::*;
use pyo3::types::{
    PyAny, PyAnyMethods, PyBool, PyByteArray, PyBytes, PyComplex, PyComplexMethods, PyDict,
    PyFloat, PyFrozenSet, PyInt, PyList, PyMemoryView, PyRange, PyRangeMethods, PySet, PyString,
    PyTuple, PyType, PyTypeMethods,
};
use runtime_tracing::{
    FieldTypeRecord, NonStreamingTraceWriter, TraceWriter, TypeId, TypeKind, TypeRecord,
    TypeSpecificInfo, ValueRecord, NONE_VALUE,
};
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::marker::PhantomData;

const DEFAULT_MAX_DEPTH: usize = 32;
const DEFAULT_MAX_SEQUENCE_ITEMS: usize = 64;
const STRING_PREVIEW_LIMIT: usize = 256;
const BINARY_PREVIEW_BYTES: usize = 1024;
const STRING_PREVIEW_TYPE: &str = "codetracer.string-preview";
const BYTES_PREVIEW_TYPE: &str = "codetracer.bytes-preview";
const DEFAULT_MAX_SET_PREVIEW: usize = 8;

#[derive(Copy, Clone)]
enum DateTimeKind {
    DateTime,
    Date,
    Time,
    Timedelta,
    Timezone,
}

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
        ValueHandler::new(guard_float, encode_float),
        ValueHandler::new(guard_complex, encode_complex),
        ValueHandler::new(guard_decimal, encode_decimal),
        ValueHandler::new(guard_fraction, encode_fraction),
        ValueHandler::new(guard_string, encode_string),
        ValueHandler::new(guard_bytes_like, encode_bytes_like),
        ValueHandler::new(guard_path_like, encode_path_like),
        ValueHandler::new(guard_dataclass, encode_dataclass),
        ValueHandler::new(guard_attrs, encode_attrs),
        ValueHandler::new(guard_namedtuple, encode_namedtuple),
        ValueHandler::new(guard_enum, encode_enum),
        ValueHandler::new(guard_simple_namespace, encode_simple_namespace),
        ValueHandler::new(guard_datetime_like, encode_datetime_like),
        ValueHandler::new(guard_tuple, encode_tuple),
        ValueHandler::new(guard_list, encode_list),
        ValueHandler::new(guard_dict, encode_dict),
        ValueHandler::new(guard_set, encode_set),
        ValueHandler::new(guard_frozenset, encode_frozenset),
        ValueHandler::new(guard_range, encode_range),
        ValueHandler::new(guard_deque, encode_deque),
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

    fn ensure_type(&mut self, kind: TypeKind, name: &str) -> TypeId {
        TraceWriter::ensure_type_id(self.writer, kind, name)
    }

    fn ensure_struct_type(&mut self, name: &str, fields: &[(&str, TypeId)]) -> TypeId {
        let dynamic_fields: Vec<(String, TypeId)> = fields
            .iter()
            .map(|(field_name, type_id)| ((*field_name).to_string(), *type_id))
            .collect();
        self.ensure_struct_type_dynamic(name, &dynamic_fields)
    }

    fn ensure_struct_type_dynamic(&mut self, name: &str, fields: &[(String, TypeId)]) -> TypeId {
        let specific_info = TypeSpecificInfo::Struct {
            fields: fields
                .iter()
                .map(|(field_name, type_id)| FieldTypeRecord {
                    name: field_name.clone(),
                    type_id: *type_id,
                })
                .collect(),
        };
        let record = TypeRecord {
            kind: TypeKind::Struct,
            lang_type: name.to_string(),
            specific_info,
        };
        TraceWriter::ensure_raw_type_id(self.writer, record)
    }

    fn encode_repr(&mut self, value: &Bound<'py, PyAny>) -> ValueRecord {
        let qualified = qualified_type_name(value).unwrap_or_else(|| "builtins.object".to_string());
        let ty = self.ensure_type(TypeKind::Raw, &qualified);
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
            || value.downcast::<PySet>().is_ok()
            || value.downcast::<PyFrozenSet>().is_ok()
    }

    fn is_mutable(&self, value: &Bound<'_, PyAny>) -> bool {
        value.downcast::<PyList>().is_ok()
            || value.downcast::<PyDict>().is_ok()
            || value.downcast::<PySet>().is_ok()
    }

    fn make_reference(
        &mut self,
        ptr: usize,
        value: &Bound<'_, PyAny>,
        canonical: ValueRecord,
    ) -> ValueRecord {
        let type_id = record_type_id(&canonical)
            .unwrap_or_else(|| self.ensure_type(TypeKind::Raw, "builtins.object"));
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
    let ty = ctx.ensure_type(TypeKind::Bool, "builtins.bool");
    let b = value.extract::<bool>().unwrap_or(false);
    ValueRecord::Bool { b, type_id: ty }
}

fn guard_int(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyInt>().is_ok()
}

fn encode_int<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let ty = ctx.ensure_type(TypeKind::Int, "builtins.int");
    if let Ok(i) = value.extract::<i64>() {
        return ValueRecord::Int { i, type_id: ty };
    }

    let int_obj = value.downcast::<PyInt>().expect("guard ensures PyInt");
    let negative = match int_obj.lt(0) {
        Ok(flag) => flag,
        Err(_) => return ctx.encode_repr(value),
    };

    let abs_obj = match int_obj.abs() {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };

    let bit_length: usize = match abs_obj
        .call_method0("bit_length")
        .and_then(|bits| bits.extract::<usize>())
    {
        Ok(bits) => bits,
        Err(_) => return ctx.encode_repr(value),
    };

    let byte_len = (bit_length + 7) / 8;
    let py_bytes_obj = match abs_obj.call_method1("to_bytes", (byte_len, "big")) {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };
    let py_bytes = match py_bytes_obj.downcast::<PyBytes>() {
        Ok(bytes) => bytes,
        Err(_) => return ctx.encode_repr(value),
    };
    let digits = py_bytes.as_bytes().to_vec();

    ValueRecord::BigInt {
        b: digits,
        negative,
        type_id: ty,
    }
}

fn guard_float(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyFloat>().is_ok()
}

fn encode_float<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let ty = ctx.ensure_type(TypeKind::Float, "builtins.float");
    match value.extract::<f64>() {
        Ok(f) => ValueRecord::Float { f, type_id: ty },
        Err(_) => ctx.encode_repr(value),
    }
}

fn guard_complex(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyComplex>().is_ok()
}

fn encode_complex<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let complex = value
        .downcast::<PyComplex>()
        .expect("guard ensures PyComplex");
    let float_ty = ctx.ensure_type(TypeKind::Float, "builtins.float");
    let real = ValueRecord::Float {
        f: complex.real(),
        type_id: float_ty,
    };
    let imag = ValueRecord::Float {
        f: complex.imag(),
        type_id: float_ty,
    };
    let ty = ctx.ensure_type(TypeKind::Tuple, "builtins.complex");
    ValueRecord::Tuple {
        elements: vec![real, imag],
        type_id: ty,
    }
}

fn guard_decimal(value: &Bound<'_, PyAny>) -> bool {
    matches_type(value, "decimal", "Decimal")
}

fn encode_decimal<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let tuple_obj = match value.call_method0("as_tuple") {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };
    let tuple = match tuple_obj.downcast::<PyTuple>() {
        Ok(tuple) => tuple,
        Err(_) => return ctx.encode_repr(value),
    };
    if tuple.len() != 3 {
        return ctx.encode_repr(value);
    }

    let sign = match tuple.get_item(0).and_then(|item| item.extract::<i32>()) {
        Ok(sign) => sign,
        Err(_) => return ctx.encode_repr(value),
    };
    let digits_obj = match tuple.get_item(1) {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };
    let digits_tuple = match digits_obj.downcast::<PyTuple>() {
        Ok(digits) => digits,
        Err(_) => return ctx.encode_repr(value),
    };
    let exponent = match tuple.get_item(2).and_then(|item| item.extract::<i64>()) {
        Ok(exp) => exp,
        Err(_) => return ctx.encode_repr(value),
    };

    let mut digits = String::new();
    for digit in digits_tuple.iter() {
        let text = match digit.str() {
            Ok(text) => text.to_string_lossy().into_owned(),
            Err(_) => return ctx.encode_repr(value),
        };
        digits.push_str(&text);
    }

    let int_ty = ctx.ensure_type(TypeKind::Int, "builtins.int");
    let str_ty = ctx.ensure_type(TypeKind::String, "builtins.str");
    let decimal_ty = ctx.ensure_struct_type(
        "decimal.Decimal",
        &[("sign", int_ty), ("digits", str_ty), ("exponent", int_ty)],
    );

    let sign_value = if sign == 0 { 1 } else { -1 };
    let sign_record = ValueRecord::Int {
        i: sign_value,
        type_id: int_ty,
    };
    let digits_record = ValueRecord::String {
        text: digits,
        type_id: str_ty,
    };
    let exponent_record = ValueRecord::Int {
        i: exponent,
        type_id: int_ty,
    };

    ValueRecord::Struct {
        field_values: vec![sign_record, digits_record, exponent_record],
        type_id: decimal_ty,
    }
}

fn guard_fraction(value: &Bound<'_, PyAny>) -> bool {
    matches_type(value, "fractions", "Fraction")
}

fn encode_fraction<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let numerator = match value.getattr("numerator") {
        Ok(val) => val,
        Err(_) => return ctx.encode_repr(value),
    };
    let denominator = match value.getattr("denominator") {
        Ok(val) => val,
        Err(_) => return ctx.encode_repr(value),
    };

    let numerator_record = ctx.encode_nested(&numerator);
    let denominator_record = ctx.encode_nested(&denominator);
    let int_ty = ctx.ensure_type(TypeKind::Int, "builtins.int");
    let fraction_ty = ctx.ensure_struct_type(
        "fractions.Fraction",
        &[("numerator", int_ty), ("denominator", int_ty)],
    );

    ValueRecord::Struct {
        field_values: vec![numerator_record, denominator_record],
        type_id: fraction_ty,
    }
}

fn guard_string(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyString>().is_ok()
}

fn encode_string<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let text = value.extract::<String>().unwrap_or_else(|_| String::new());
    encode_text_value(ctx, "builtins.str", text)
}

fn guard_bytes_like(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyBytes>().is_ok()
        || value.downcast::<PyByteArray>().is_ok()
        || value.downcast::<PyMemoryView>().is_ok()
}

fn encode_bytes_like<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    if let Ok(bytes) = value.downcast::<PyBytes>() {
        return encode_bytes_preview(ctx, bytes.as_bytes(), "builtins.bytes");
    }

    if let Ok(bytearray) = value.downcast::<PyByteArray>() {
        let data = match bytearray.extract::<Vec<u8>>() {
            Ok(buf) => buf,
            Err(_) => return ctx.encode_repr(value),
        };
        return encode_bytes_preview(ctx, &data, "builtins.bytearray");
    }

    if let Ok(mem) = value.downcast::<PyMemoryView>() {
        let data = match mem.extract::<Vec<u8>>() {
            Ok(buf) => buf,
            Err(_) => return ctx.encode_repr(value),
        };
        return encode_bytes_preview(ctx, &data, "builtins.memoryview");
    }

    ctx.encode_repr(value)
}

fn guard_path_like(value: &Bound<'_, PyAny>) -> bool {
    if value.downcast::<PyString>().is_ok() || value.downcast::<PyBytes>().is_ok() {
        return false;
    }
    value.hasattr("__fspath__").unwrap_or(false)
}

fn encode_path_like<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let path_value = match value.call_method0("__fspath__") {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };

    let type_name = qualified_type_name(value).unwrap_or_else(|| "os.PathLike".to_string());
    if let Ok(text) = path_value.extract::<String>() {
        return encode_text_value(ctx, &type_name, text);
    }
    if let Ok(bytes) = path_value.downcast::<PyBytes>() {
        return encode_bytes_preview(ctx, bytes.as_bytes(), &type_name);
    }
    ctx.encode_repr(value)
}

fn guard_dataclass(value: &Bound<'_, PyAny>) -> bool {
    value.hasattr("__dataclass_fields__").unwrap_or(false)
}

fn encode_dataclass<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let fields_obj = match value.getattr("__dataclass_fields__") {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };
    let fields = match fields_obj.downcast::<PyDict>() {
        Ok(dict) => dict,
        Err(_) => return ctx.encode_repr(value),
    };
    let mut records = Vec::new();
    for (key, _) in fields.iter() {
        let name: String = match key.extract() {
            Ok(name) => name,
            Err(_) => return ctx.encode_repr(value),
        };
        let attr_value = match value.getattr(name.as_str()) {
            Ok(val) => val,
            Err(_) => return ctx.encode_repr(value),
        };
        records.push((name, ctx.encode_nested(&attr_value)));
    }
    let type_name = qualified_type_name(value).unwrap_or_else(|| "dataclass".to_string());
    encode_struct_from_records(ctx, &type_name, records)
}

fn guard_attrs(value: &Bound<'_, PyAny>) -> bool {
    value.hasattr("__attrs_attrs__").unwrap_or(false)
}

fn encode_attrs<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let attrs_obj = match value.getattr("__attrs_attrs__") {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };
    let attrs = match attrs_obj.downcast::<PyTuple>() {
        Ok(tuple) => tuple,
        Err(_) => return ctx.encode_repr(value),
    };
    let mut records = Vec::new();
    for attr in attrs.iter() {
        let name: String = match attr.getattr("name").and_then(|n| n.extract::<String>()) {
            Ok(name) => name,
            Err(_) => return ctx.encode_repr(value),
        };
        let attr_value = match value.getattr(name.as_str()) {
            Ok(val) => val,
            Err(_) => return ctx.encode_repr(value),
        };
        records.push((name, ctx.encode_nested(&attr_value)));
    }
    let type_name = qualified_type_name(value).unwrap_or_else(|| "attrs".to_string());
    encode_struct_from_records(ctx, &type_name, records)
}

fn guard_namedtuple(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyTuple>().is_ok() && value.hasattr("_fields").unwrap_or(false)
}

fn encode_namedtuple<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let fields_obj = match value.getattr("_fields") {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };
    let fields = match fields_obj.downcast::<PyTuple>() {
        Ok(tuple) => tuple,
        Err(_) => return ctx.encode_repr(value),
    };
    let mut records = Vec::new();
    for field in fields.iter() {
        let name: String = match field.extract() {
            Ok(name) => name,
            Err(_) => return ctx.encode_repr(value),
        };
        let attr_value = match value.getattr(name.as_str()) {
            Ok(val) => val,
            Err(_) => return ctx.encode_repr(value),
        };
        records.push((name, ctx.encode_nested(&attr_value)));
    }
    let type_name = qualified_type_name(value).unwrap_or_else(|| "namedtuple".to_string());
    encode_struct_from_records(ctx, &type_name, records)
}

fn guard_enum(value: &Bound<'_, PyAny>) -> bool {
    let ty = value.get_type();
    is_enum_type(&ty)
}

fn encode_enum<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let str_ty = ctx.ensure_type(TypeKind::String, "builtins.str");
    let name = match value.getattr("name").and_then(|n| n.extract::<String>()) {
        Ok(name) => name,
        Err(_) => return ctx.encode_repr(value),
    };
    let enum_value = match value.getattr("value") {
        Ok(val) => val,
        Err(_) => return ctx.encode_repr(value),
    };
    let mut records = Vec::new();
    records.push((
        "name".to_string(),
        ValueRecord::String {
            text: name,
            type_id: str_ty,
        },
    ));
    records.push(("value".to_string(), ctx.encode_nested(&enum_value)));
    let type_name = qualified_type_name(value).unwrap_or_else(|| "enum.Enum".to_string());
    encode_struct_from_records(ctx, &type_name, records)
}

fn guard_simple_namespace(value: &Bound<'_, PyAny>) -> bool {
    matches!(
        qualified_type_name(value).as_deref(),
        Some("types.SimpleNamespace")
    )
}

fn encode_simple_namespace<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let dict_obj = match value.getattr("__dict__") {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };
    let dict = match dict_obj.downcast::<PyDict>() {
        Ok(dict) => dict,
        Err(_) => return ctx.encode_repr(value),
    };
    let mut records = Vec::new();
    for (key, val) in dict.iter() {
        let name: String = match key.extract() {
            Ok(name) => name,
            Err(_) => return ctx.encode_repr(value),
        };
        if name.starts_with('_') {
            continue;
        }
        records.push((name, ctx.encode_nested(&val)));
    }
    records.sort_by(|a, b| a.0.cmp(&b.0));
    let type_name =
        qualified_type_name(value).unwrap_or_else(|| "types.SimpleNamespace".to_string());
    encode_struct_from_records(ctx, &type_name, records)
}

fn guard_datetime_like(value: &Bound<'_, PyAny>) -> bool {
    datetime_kind(value).is_some()
}

fn encode_datetime_like<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let kind = match datetime_kind(value) {
        Some(kind) => kind,
        None => return ctx.encode_repr(value),
    };
    let py = value.py();
    let str_ty = ctx.ensure_type(TypeKind::String, "builtins.str");
    let int_ty = ctx.ensure_type(TypeKind::Int, "builtins.int");
    let float_ty = ctx.ensure_type(TypeKind::Float, "builtins.float");

    match kind {
        DateTimeKind::DateTime => {
            let iso = match value
                .call_method0("isoformat")
                .and_then(|s| s.extract::<String>())
            {
                Ok(iso) => iso,
                Err(_) => return ctx.encode_repr(value),
            };
            let timestamp = match value
                .call_method0("timestamp")
                .and_then(|ts| ts.extract::<f64>())
            {
                Ok(ts) => ts,
                Err(_) => return ctx.encode_repr(value),
            };
            let tzinfo = match value.getattr("tzinfo") {
                Ok(tz) => tz,
                Err(_) => return ctx.encode_repr(value),
            };
            let fields = vec![
                (
                    "isoformat".to_string(),
                    ValueRecord::String {
                        text: iso,
                        type_id: str_ty,
                    },
                ),
                (
                    "timestamp".to_string(),
                    ValueRecord::Float {
                        f: timestamp,
                        type_id: float_ty,
                    },
                ),
                ("tzinfo".to_string(), ctx.encode_nested(&tzinfo)),
            ];
            encode_struct_from_records(ctx, "datetime.datetime", fields)
        }
        DateTimeKind::Date => {
            let iso = match value
                .call_method0("isoformat")
                .and_then(|s| s.extract::<String>())
            {
                Ok(iso) => iso,
                Err(_) => return ctx.encode_repr(value),
            };
            let year = match value.getattr("year").and_then(|v| v.extract::<i64>()) {
                Ok(year) => year,
                Err(_) => return ctx.encode_repr(value),
            };
            let month = match value.getattr("month").and_then(|v| v.extract::<i64>()) {
                Ok(month) => month,
                Err(_) => return ctx.encode_repr(value),
            };
            let day = match value.getattr("day").and_then(|v| v.extract::<i64>()) {
                Ok(day) => day,
                Err(_) => return ctx.encode_repr(value),
            };
            let fields = vec![
                (
                    "isoformat".to_string(),
                    ValueRecord::String {
                        text: iso,
                        type_id: str_ty,
                    },
                ),
                (
                    "year".to_string(),
                    ValueRecord::Int {
                        i: year,
                        type_id: int_ty,
                    },
                ),
                (
                    "month".to_string(),
                    ValueRecord::Int {
                        i: month,
                        type_id: int_ty,
                    },
                ),
                (
                    "day".to_string(),
                    ValueRecord::Int {
                        i: day,
                        type_id: int_ty,
                    },
                ),
            ];
            encode_struct_from_records(ctx, "datetime.date", fields)
        }
        DateTimeKind::Time => {
            let iso = match value
                .call_method0("isoformat")
                .and_then(|s| s.extract::<String>())
            {
                Ok(iso) => iso,
                Err(_) => return ctx.encode_repr(value),
            };
            let hour = match value.getattr("hour").and_then(|v| v.extract::<i64>()) {
                Ok(hour) => hour,
                Err(_) => return ctx.encode_repr(value),
            };
            let minute = match value.getattr("minute").and_then(|v| v.extract::<i64>()) {
                Ok(minute) => minute,
                Err(_) => return ctx.encode_repr(value),
            };
            let second = match value.getattr("second").and_then(|v| v.extract::<i64>()) {
                Ok(second) => second,
                Err(_) => return ctx.encode_repr(value),
            };
            let microsecond = match value
                .getattr("microsecond")
                .and_then(|v| v.extract::<i64>())
            {
                Ok(us) => us,
                Err(_) => return ctx.encode_repr(value),
            };
            let tzinfo = match value.getattr("tzinfo") {
                Ok(tz) => tz,
                Err(_) => return ctx.encode_repr(value),
            };
            let fields = vec![
                (
                    "isoformat".to_string(),
                    ValueRecord::String {
                        text: iso,
                        type_id: str_ty,
                    },
                ),
                (
                    "hour".to_string(),
                    ValueRecord::Int {
                        i: hour,
                        type_id: int_ty,
                    },
                ),
                (
                    "minute".to_string(),
                    ValueRecord::Int {
                        i: minute,
                        type_id: int_ty,
                    },
                ),
                (
                    "second".to_string(),
                    ValueRecord::Int {
                        i: second,
                        type_id: int_ty,
                    },
                ),
                (
                    "microsecond".to_string(),
                    ValueRecord::Int {
                        i: microsecond,
                        type_id: int_ty,
                    },
                ),
                ("tzinfo".to_string(), ctx.encode_nested(&tzinfo)),
            ];
            encode_struct_from_records(ctx, "datetime.time", fields)
        }
        DateTimeKind::Timedelta => {
            let days = match value.getattr("days").and_then(|v| v.extract::<i64>()) {
                Ok(days) => days,
                Err(_) => return ctx.encode_repr(value),
            };
            let seconds = match value.getattr("seconds").and_then(|v| v.extract::<i64>()) {
                Ok(seconds) => seconds,
                Err(_) => return ctx.encode_repr(value),
            };
            let microseconds = match value
                .getattr("microseconds")
                .and_then(|v| v.extract::<i64>())
            {
                Ok(us) => us,
                Err(_) => return ctx.encode_repr(value),
            };
            let total_seconds = match value
                .call_method0("total_seconds")
                .and_then(|ts| ts.extract::<f64>())
            {
                Ok(ts) => ts,
                Err(_) => return ctx.encode_repr(value),
            };
            let fields = vec![
                (
                    "days".to_string(),
                    ValueRecord::Int {
                        i: days,
                        type_id: int_ty,
                    },
                ),
                (
                    "seconds".to_string(),
                    ValueRecord::Int {
                        i: seconds,
                        type_id: int_ty,
                    },
                ),
                (
                    "microseconds".to_string(),
                    ValueRecord::Int {
                        i: microseconds,
                        type_id: int_ty,
                    },
                ),
                (
                    "total_seconds".to_string(),
                    ValueRecord::Float {
                        f: total_seconds,
                        type_id: float_ty,
                    },
                ),
            ];
            encode_struct_from_records(ctx, "datetime.timedelta", fields)
        }
        DateTimeKind::Timezone => {
            let name = match value
                .call_method1("tzname", (py.None(),))
                .and_then(|n| n.extract::<String>())
            {
                Ok(name) => name,
                Err(_) => return ctx.encode_repr(value),
            };
            let offset = match value.call_method1("utcoffset", (py.None(),)) {
                Ok(obj) => ctx.encode_nested(&obj),
                Err(_) => return ctx.encode_repr(value),
            };
            let fields = vec![
                (
                    "name".to_string(),
                    ValueRecord::String {
                        text: name,
                        type_id: str_ty,
                    },
                ),
                ("offset".to_string(), offset),
            ];
            encode_struct_from_records(ctx, "datetime.timezone", fields)
        }
    }
}

fn guard_set(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PySet>().is_ok()
}

fn encode_set<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    encode_set_like(ctx, value, true)
}

fn guard_frozenset(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyFrozenSet>().is_ok()
}

fn encode_frozenset<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    encode_set_like(ctx, value, true)
}

fn guard_range(value: &Bound<'_, PyAny>) -> bool {
    value.downcast::<PyRange>().is_ok()
}

fn encode_range<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    let range = value.downcast::<PyRange>().expect("guard ensures range");
    let start = range.start().ok().map(convert_isize).unwrap_or(0);
    let stop = range.stop().ok().map(convert_isize).unwrap_or(0);
    let step = range.step().ok().map(convert_isize).unwrap_or(1);

    let int_ty = ctx.ensure_type(TypeKind::Int, "builtins.int");
    let range_ty = ctx.ensure_struct_type(
        "codetracer.range",
        &[("start", int_ty), ("stop", int_ty), ("step", int_ty)],
    );

    ValueRecord::Struct {
        field_values: vec![
            ValueRecord::Int {
                i: start,
                type_id: int_ty,
            },
            ValueRecord::Int {
                i: stop,
                type_id: int_ty,
            },
            ValueRecord::Int {
                i: step,
                type_id: int_ty,
            },
        ],
        type_id: range_ty,
    }
}

fn guard_deque(value: &Bound<'_, PyAny>) -> bool {
    matches_type(value, "collections", "deque")
}

fn encode_deque<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
) -> ValueRecord {
    encode_iterable_sequence(ctx, value, "collections.deque")
}

fn encode_text_value<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    type_name: &str,
    text: String,
) -> ValueRecord {
    let char_count = text.chars().count();
    if char_count <= STRING_PREVIEW_LIMIT {
        let ty = ctx.ensure_type(TypeKind::String, type_name);
        return ValueRecord::String { text, type_id: ty };
    }

    let mut preview_end_bytes = text.len();
    let mut consumed = 0usize;
    for (idx, ch) in text.char_indices() {
        consumed += 1;
        let next_idx = idx + ch.len_utf8();
        if consumed == STRING_PREVIEW_LIMIT {
            preview_end_bytes = next_idx;
            break;
        }
    }

    let preview = text[..preview_end_bytes].to_string();
    let str_ty = ctx.ensure_type(TypeKind::String, "builtins.str");
    let int_ty = ctx.ensure_type(TypeKind::Int, "builtins.int");
    let bool_ty = ctx.ensure_type(TypeKind::Bool, "builtins.bool");
    let struct_name = string_preview_type(type_name);
    let struct_ty = ctx.ensure_struct_type(
        &struct_name,
        &[
            ("preview", str_ty),
            ("total_length", int_ty),
            ("truncated", bool_ty),
        ],
    );
    let total_len = char_count.try_into().unwrap_or(i64::MAX);

    ValueRecord::Struct {
        field_values: vec![
            ValueRecord::String {
                text: preview,
                type_id: str_ty,
            },
            ValueRecord::Int {
                i: total_len,
                type_id: int_ty,
            },
            ValueRecord::Bool {
                b: true,
                type_id: bool_ty,
            },
        ],
        type_id: struct_ty,
    }
}

fn string_preview_type(type_name: &str) -> String {
    if type_name == "builtins.str" {
        STRING_PREVIEW_TYPE.to_string()
    } else {
        format!("{type_name}#string-preview")
    }
}

fn encode_bytes_preview<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    data: &[u8],
    type_name: &str,
) -> ValueRecord {
    let total_len = data.len();
    let preview_len = total_len.min(BINARY_PREVIEW_BYTES);
    let preview_slice = &data[..preview_len];
    let preview_b64 = BASE64_STANDARD.encode(preview_slice);
    let truncated = total_len > BINARY_PREVIEW_BYTES;

    let raw_ty = ctx.ensure_type(TypeKind::Raw, type_name);
    let int_ty = ctx.ensure_type(TypeKind::Int, "builtins.int");
    let bool_ty = ctx.ensure_type(TypeKind::Bool, "builtins.bool");
    let struct_ty = ctx.ensure_struct_type(
        BYTES_PREVIEW_TYPE,
        &[
            ("preview_b64", raw_ty),
            ("total_bytes", int_ty),
            ("truncated", bool_ty),
        ],
    );
    let total_i64 = total_len.try_into().unwrap_or(i64::MAX);

    ValueRecord::Struct {
        field_values: vec![
            ValueRecord::Raw {
                r: preview_b64,
                type_id: raw_ty,
            },
            ValueRecord::Int {
                i: total_i64,
                type_id: int_ty,
            },
            ValueRecord::Bool {
                b: truncated,
                type_id: bool_ty,
            },
        ],
        type_id: struct_ty,
    }
}

fn encode_struct_from_records<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    type_name: &str,
    fields: Vec<(String, ValueRecord)>,
) -> ValueRecord {
    let mut spec = Vec::with_capacity(fields.len());
    let mut values = Vec::with_capacity(fields.len());
    for (name, record) in fields {
        let type_id = record_type_id(&record)
            .unwrap_or_else(|| ctx.ensure_type(TypeKind::Raw, "builtins.object"));
        spec.push((name, type_id));
        values.push(record);
    }
    let type_id = ctx.ensure_struct_type_dynamic(type_name, &spec);
    ValueRecord::Struct {
        field_values: values,
        type_id,
    }
}

fn encode_set_like<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
    unordered: bool,
) -> ValueRecord {
    let limit = ctx.limits.max_items.min(DEFAULT_MAX_SET_PREVIEW);
    let py = value.py();
    let sorted_list = match pyo3::types::PyModule::import(py, "builtins")
        .and_then(|builtins| builtins.getattr("sorted"))
        .and_then(|sorted_fn| sorted_fn.call1((value,)))
    {
        Ok(obj) => obj,
        Err(_) => return ctx.encode_repr(value),
    };
    let sorted_list = match sorted_list.downcast::<PyList>() {
        Ok(list) => list,
        Err(_) => return ctx.encode_repr(value),
    };

    let total_count = sorted_list.len();
    let mut preview = Vec::with_capacity(limit);
    for (index, item) in sorted_list.iter().enumerate() {
        if index < limit {
            preview.push(ctx.encode_nested(&item));
        } else {
            preview.shrink_to_fit();
            break;
        }
    }
    let truncated = total_count > preview.len();

    let preview_type = ctx.ensure_type(TypeKind::Seq, "builtins.list");
    let int_ty = ctx.ensure_type(TypeKind::Int, "builtins.int");
    let bool_ty = ctx.ensure_type(TypeKind::Bool, "builtins.bool");
    let metadata_ty = ctx.ensure_struct_type(
        "codetracer.set-metadata",
        &[
            ("preview", preview_type),
            ("total_count", int_ty),
            ("unordered", bool_ty),
        ],
    );

    let preview_record = ValueRecord::Sequence {
        elements: preview,
        is_slice: truncated,
        type_id: preview_type,
    };

    ValueRecord::Struct {
        field_values: vec![
            preview_record,
            ValueRecord::Int {
                i: total_count.try_into().unwrap_or(i64::MAX),
                type_id: int_ty,
            },
            ValueRecord::Bool {
                b: unordered,
                type_id: bool_ty,
            },
        ],
        type_id: metadata_ty,
    }
}

fn encode_iterable_sequence<'py>(
    ctx: &mut ValueEncoderContext<'py, '_>,
    value: &Bound<'py, PyAny>,
    type_name: &str,
) -> ValueRecord {
    let mut iter = match value.try_iter() {
        Ok(iter) => iter,
        Err(_) => return ctx.encode_repr(value),
    };

    let mut elements = Vec::new();
    let limit = ctx.limits.max_items;
    let mut truncated = false;

    while let Some(result) = iter.next() {
        let item = match result {
            Ok(item) => item,
            Err(_) => return ctx.encode_repr(value),
        };
        if elements.len() < limit {
            elements.push(ctx.encode_nested(&item));
        } else {
            truncated = true;
            break;
        }
    }

    if !truncated {
        if let Ok(len) = value.len() {
            truncated = len > elements.len();
        }
    }

    let ty = ctx.ensure_type(TypeKind::Seq, type_name);
    ValueRecord::Sequence {
        elements,
        is_slice: truncated,
        type_id: ty,
    }
}

fn convert_isize(value: isize) -> i64 {
    i64::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        }
    })
}

fn is_enum_type(ty: &Bound<'_, PyType>) -> bool {
    let mro = ty.mro();
    for base in mro.iter() {
        if let Ok(base_type) = base.downcast::<PyType>() {
            let name_matches = base_type
                .name()
                .map(|name| name.to_string_lossy() == "Enum")
                .unwrap_or(false);
            if !name_matches {
                continue;
            }
            let module_matches = base_type
                .module()
                .map(|module| module.to_string_lossy() == "enum")
                .unwrap_or(false);
            if module_matches {
                return true;
            }
        }
    }
    false
}

fn datetime_kind(value: &Bound<'_, PyAny>) -> Option<DateTimeKind> {
    let ty = value.get_type();
    let name = ty.name().ok()?.to_string_lossy().into_owned();
    let module = ty
        .module()
        .ok()
        .map(|module| module.to_string_lossy().into_owned())
        .unwrap_or_default();
    if module != "datetime" {
        return None;
    }
    match name.as_str() {
        "datetime" => Some(DateTimeKind::DateTime),
        "date" => Some(DateTimeKind::Date),
        "time" => Some(DateTimeKind::Time),
        "timedelta" => Some(DateTimeKind::Timedelta),
        "timezone" => Some(DateTimeKind::Timezone),
        _ => None,
    }
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
    let ty = ctx.ensure_type(TypeKind::Tuple, "builtins.tuple");
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
    let ty = ctx.ensure_type(TypeKind::Seq, "builtins.list");
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
    let tuple_ty = ctx.ensure_type(TypeKind::Tuple, "builtins.tuple");
    let str_ty = ctx.ensure_type(TypeKind::String, "builtins.str");
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
    let ty = ctx.ensure_type(TypeKind::Seq, "builtins.dict");
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

fn qualified_type_name(value: &Bound<'_, PyAny>) -> Option<String> {
    let ty = value.get_type();
    let name = ty.name().ok()?;
    let name = name.to_string_lossy().into_owned();
    match ty.module() {
        Ok(module) => {
            let module_cow = module.to_string_lossy();
            let module_owned = if module_cow.starts_with("pathlib.") {
                "pathlib".to_string()
            } else {
                module_cow.into_owned()
            };
            if module_owned.is_empty() {
                Some(name)
            } else {
                Some(format!("{}.{}", module_owned, name))
            }
        }
        Err(_) => Some(name),
    }
}

fn matches_type(value: &Bound<'_, PyAny>, module: &str, name: &str) -> bool {
    let ty = value.get_type();
    match (ty.module(), ty.name()) {
        (Ok(module_name), Ok(type_name)) => {
            module_name.to_string_lossy() == module && type_name.to_string_lossy() == name
        }
        (Ok(module_name), Err(_)) => module_name.to_string_lossy() == module && name.is_empty(),
        _ => false,
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
    use pyo3::types::{PyList, PyModule, PyTuple};
    use runtime_tracing::{Line, NonStreamingTraceWriter, TraceWriter, TypeKind, ValueRecord};
    use std::ffi::{CStr, CString};

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

    fn value_from_code<'py>(py: Python<'py>, code: &str) -> pyo3::PyResult<Bound<'py, PyAny>> {
        let code_cstr = CString::new(code).expect("code literal");
        let filename = CStr::from_bytes_with_nul(b"<test>\0").unwrap();
        let module_name = CStr::from_bytes_with_nul(b"<test_module>\0").unwrap();
        let module = PyModule::from_code(py, code_cstr.as_c_str(), filename, module_name)?;
        module.getattr("value")
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
    fn big_int_encoding_emits_bigint_variant() {
        Python::with_gil(|py| {
            let expr = CString::new("1 << 80").expect("big int literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::BigInt { negative, b, .. } => {
                    assert!(!negative, "expected positive bigint");
                    assert_eq!(b, vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                }
                other => panic!("expected BigInt record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn float_handler_encodes_literal() {
        Python::with_gil(|py| {
            let expr = CString::new("3.5").expect("float literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Float { f, .. } => assert!((f - 3.5).abs() < f64::EPSILON),
                other => panic!("expected Float record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn complex_handler_emits_real_imag_tuple() {
        Python::with_gil(|py| {
            let expr = CString::new("complex(1.5, -2.25)").expect("complex literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Tuple { elements, .. } => {
                    assert_eq!(elements.len(), 2);
                    match (&elements[0], &elements[1]) {
                        (
                            ValueRecord::Float { f: real, .. },
                            ValueRecord::Float { f: imag, .. },
                        ) => {
                            assert!((*real - 1.5).abs() < f64::EPSILON);
                            assert!((*imag + 2.25).abs() < f64::EPSILON);
                        }
                        other => panic!("expected float elements, saw {:?}", other),
                    }
                }
                other => panic!("expected Tuple record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn decimal_handler_encodes_struct_fields() {
        Python::with_gil(|py| {
            let expr =
                CString::new("__import__('decimal').Decimal('-12.34')").expect("decimal literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct { field_values, .. } => {
                    assert_eq!(field_values.len(), 3);
                    match &field_values[0] {
                        ValueRecord::Int { i, .. } => assert_eq!(*i, -1),
                        other => panic!("expected sign int, saw {:?}", other),
                    }
                    match &field_values[1] {
                        ValueRecord::String { text, .. } => assert_eq!(text, "1234"),
                        other => panic!("expected digits string, saw {:?}", other),
                    }
                    match &field_values[2] {
                        ValueRecord::Int { i, .. } => assert_eq!(*i, -2),
                        other => panic!("expected exponent int, saw {:?}", other),
                    }
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn fraction_handler_encodes_numerator_denominator() {
        Python::with_gil(|py| {
            let expr =
                CString::new("__import__('fractions').Fraction(-3, 4)").expect("fraction literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct { field_values, .. } => {
                    assert_eq!(field_values.len(), 2);
                    match &field_values[0] {
                        ValueRecord::Int { i, .. } => assert_eq!(*i, -3),
                        other => panic!("expected numerator int, saw {:?}", other),
                    }
                    match &field_values[1] {
                        ValueRecord::Int { i, .. } => assert_eq!(*i, 4),
                        other => panic!("expected denominator int, saw {:?}", other),
                    }
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn bytearray_handler_emits_preview_struct() {
        Python::with_gil(|py| {
            let expr = CString::new("bytearray(b'B' * 1500)").expect("bytearray literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct { field_values, .. } => {
                    assert_eq!(field_values.len(), 3);
                    match &field_values[0] {
                        ValueRecord::Raw { r, .. } => assert!(r.starts_with("QkJCQkJC")),
                        other => panic!("expected raw preview, saw {:?}", other),
                    }
                    match &field_values[1] {
                        ValueRecord::Int { i, .. } => assert_eq!(*i, 1500),
                        other => panic!("expected total bytes int, saw {:?}", other),
                    }
                    match &field_values[2] {
                        ValueRecord::Bool { b, .. } => assert!(*b),
                        other => panic!("expected truncated bool, saw {:?}", other),
                    }
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn set_handler_emits_metadata_struct() {
        Python::with_gil(|py| {
            let expr = CString::new("set(range(5))").expect("set literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct {
                    field_values,
                    type_id,
                } => {
                    let expected = TraceWriter::ensure_type_id(
                        &mut writer,
                        TypeKind::Struct,
                        "codetracer.set-metadata",
                    );
                    assert_eq!(type_id, expected);
                    assert_eq!(field_values.len(), 3);
                    match &field_values[0] {
                        ValueRecord::Sequence { is_slice, .. } => assert!(!is_slice),
                        other => panic!("expected preview sequence, saw {:?}", other),
                    }
                    match &field_values[1] {
                        ValueRecord::Int { i, .. } => assert_eq!(*i, 5),
                        other => panic!("expected total_count int, saw {:?}", other),
                    }
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn range_handler_encodes_bounds() {
        Python::with_gil(|py| {
            let expr = CString::new("range(1, 7, 2)").expect("range literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct { field_values, .. } => {
                    assert_eq!(field_values.len(), 3);
                    let mut ints = field_values.iter().map(|val| match val {
                        ValueRecord::Int { i, .. } => *i,
                        other => panic!("expected Int field, saw {:?}", other),
                    });
                    assert_eq!(ints.next(), Some(1));
                    assert_eq!(ints.next(), Some(7));
                    assert_eq!(ints.next(), Some(2));
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn dataclass_handler_encodes_fields() {
        Python::with_gil(|py| {
            let value = value_from_code(
                py,
                "from dataclasses import dataclass\n@dataclass\nclass Point:\n    x: int\n    y: int\nvalue = Point(10, 20)",
            )
            .expect("dataclass instance");
            let type_name = super::qualified_type_name(&value).expect("type name");
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct {
                    field_values,
                    type_id,
                } => {
                    assert_eq!(field_values.len(), 2);
                    let expected =
                        TraceWriter::ensure_type_id(&mut writer, TypeKind::Struct, &type_name);
                    assert_eq!(type_id, expected);
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn namedtuple_handler_encodes_fields() {
        Python::with_gil(|py| {
            let value = value_from_code(
                py,
                "from collections import namedtuple\nPair = namedtuple('Pair', ['a', 'b'])\nPair.__module__ = __name__\nvalue = Pair(3, 4)",
            )
            .expect("namedtuple instance");
            let type_name = super::qualified_type_name(&value).expect("type name");
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct {
                    field_values,
                    type_id,
                } => {
                    assert_eq!(field_values.len(), 2);
                    let expected =
                        TraceWriter::ensure_type_id(&mut writer, TypeKind::Struct, &type_name);
                    assert_eq!(type_id, expected);
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn enum_handler_encodes_name_and_value() {
        Python::with_gil(|py| {
            let value = value_from_code(
                py,
                "from enum import Enum\nclass Color(Enum):\n    RED = 1\nvalue = Color.RED",
            )
            .expect("enum instance");
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct { field_values, .. } => {
                    assert_eq!(field_values.len(), 2);
                    match &field_values[0] {
                        ValueRecord::String { text, .. } => assert_eq!(text, "RED"),
                        other => panic!("expected enum name string, saw {:?}", other),
                    }
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn simple_namespace_handler_orders_fields() {
        Python::with_gil(|py| {
            let expr = CString::new("__import__('types').SimpleNamespace(foo=1, bar=2)")
                .expect("namespace literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct { field_values, .. } => {
                    assert_eq!(field_values.len(), 2);
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn datetime_handler_produces_struct() {
        Python::with_gil(|py| {
            let expr = CString::new(
                "__import__('datetime').datetime(2024, 1, 2, 3, 4, 5, tzinfo=__import__('datetime').timezone.utc)",
            )
            .expect("datetime literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Struct { field_values, .. } => {
                    assert!(field_values.len() >= 3);
                }
                other => panic!("expected Struct record, saw {:?}", other),
            }
        });
    }

    #[test]
    fn deque_handler_encodes_sequence_preview() {
        Python::with_gil(|py| {
            let expr =
                CString::new("__import__('collections').deque([1, 2, 3])").expect("deque literal");
            let value = py.eval(expr.as_c_str(), None, None).unwrap();
            let mut writer = NonStreamingTraceWriter::new("<test>", &[]);
            writer.start(std::path::Path::new("<test>"), Line(1));
            let mut context = ValueEncoderContext::new(py, &mut writer);
            match context.encode_root(&value) {
                ValueRecord::Sequence {
                    elements,
                    is_slice,
                    type_id,
                } => {
                    let expected = TraceWriter::ensure_type_id(
                        &mut writer,
                        TypeKind::Seq,
                        "collections.deque",
                    );
                    assert_eq!(type_id, expected);
                    assert_eq!(elements.len(), 3);
                    assert!(!is_slice);
                }
                other => panic!("expected Sequence record, saw {:?}", other),
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
