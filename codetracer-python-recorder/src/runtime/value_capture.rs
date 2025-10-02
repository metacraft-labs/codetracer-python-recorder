//! Helpers for capturing call arguments and variable scope for tracing callbacks.

use std::collections::HashSet;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyString;

use runtime_tracing::{FullValueRecord, NonStreamingTraceWriter, TraceWriter};

use crate::code_object::CodeObjectWrapper;
use crate::runtime::frame_inspector::{capture_frame, FrameSnapshot};
use crate::runtime::value_encoder::encode_value;

/// Capture Python call arguments for the provided code object and encode them
/// using the runtime tracer writer.
pub fn capture_call_arguments<'py>(
    py: Python<'py>,
    writer: &mut NonStreamingTraceWriter,
    code: &CodeObjectWrapper,
) -> PyResult<Vec<FullValueRecord>> {
    let snapshot = capture_frame(py, code)?;
    let locals = snapshot.locals();

    let code_bound = code.as_bound(py);
    let argcount = code.arg_count(py)? as usize;
    let _posonly: usize = code_bound.getattr("co_posonlyargcount")?.extract()?;
    let kwonly: usize = code_bound.getattr("co_kwonlyargcount")?.extract()?;
    let flags = code.flags(py)?;

    const CO_VARARGS: u32 = 0x04;
    const CO_VARKEYWORDS: u32 = 0x08;

    let varnames: Vec<String> = code_bound.getattr("co_varnames")?.extract()?;

    let mut args: Vec<FullValueRecord> = Vec::new();
    let mut idx = 0usize;

    let positional_take = std::cmp::min(argcount, varnames.len());
    for name in varnames.iter().take(positional_take) {
        let value = locals
            .get_item(name)?
            .ok_or_else(|| PyRuntimeError::new_err(format!("missing positional arg '{name}'")))?;
        let encoded = encode_value(py, writer, &value);
        args.push(TraceWriter::arg(writer, name, encoded));
        idx += 1;
    }

    if (flags & CO_VARARGS) != 0 && idx < varnames.len() {
        let name = &varnames[idx];
        if let Some(value) = locals.get_item(name)? {
            let encoded = encode_value(py, writer, &value);
            args.push(TraceWriter::arg(writer, name, encoded));
        }
        idx += 1;
    }

    let kwonly_take = std::cmp::min(kwonly, varnames.len().saturating_sub(idx));
    for name in varnames.iter().skip(idx).take(kwonly_take) {
        let value = locals
            .get_item(name)?
            .ok_or_else(|| PyRuntimeError::new_err(format!("missing kw-only arg '{name}'")))?;
        let encoded = encode_value(py, writer, &value);
        args.push(TraceWriter::arg(writer, name, encoded));
    }
    idx = idx.saturating_add(kwonly_take);

    if (flags & CO_VARKEYWORDS) != 0 && idx < varnames.len() {
        let name = &varnames[idx];
        if let Some(value) = locals.get_item(name)? {
            let encoded = encode_value(py, writer, &value);
            args.push(TraceWriter::arg(writer, name, encoded));
        }
    }

    Ok(args)
}

/// Record all visible variables from the provided frame snapshot into the writer.
pub fn record_visible_scope(
    py: Python<'_>,
    writer: &mut NonStreamingTraceWriter,
    snapshot: &FrameSnapshot<'_>,
    recorded: &mut HashSet<String>,
) {
    for (key, value) in snapshot.locals().iter() {
        let name = match key.downcast::<PyString>() {
            Ok(pystr) => match pystr.to_str() {
                Ok(raw) => raw.to_owned(),
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        let encoded = encode_value(py, writer, &value);
        TraceWriter::register_variable_with_full_value(writer, &name, encoded);
        recorded.insert(name);
    }

    if snapshot.locals_is_globals() {
        return;
    }

    if let Some(globals_dict) = snapshot.globals() {
        for (key, value) in globals_dict.iter() {
            let name = match key.downcast::<PyString>() {
                Ok(pystr) => match pystr.to_str() {
                    Ok(raw) => raw,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            if name == "__builtins__" || recorded.contains(name) {
                continue;
            }
            let encoded = encode_value(py, writer, &value);
            TraceWriter::register_variable_with_full_value(writer, name, encoded);
            recorded.insert(name.to_owned());
        }
    }
}
