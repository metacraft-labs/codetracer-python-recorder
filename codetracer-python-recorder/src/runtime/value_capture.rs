//! Helpers for capturing call arguments and variable scope for tracing callbacks.

use std::collections::HashSet;

use pyo3::prelude::*;
use pyo3::types::PyString;

use recorder_errors::{usage, ErrorCode};
use runtime_tracing::{
    FullValueRecord, NonStreamingTraceWriter, TraceWriter, TypeKind, ValueRecord,
};

use crate::code_object::CodeObjectWrapper;
use crate::ffi;
use crate::logging::record_dropped_event;
use crate::runtime::frame_inspector::{capture_frame, FrameSnapshot};
use crate::runtime::value_encoder::encode_value;
use crate::trace_filter::config::ValueAction;
use crate::trace_filter::engine::{ValueKind, ValuePolicy};

const REDACTED_SENTINEL: &str = "<redacted>";

const VALUE_KIND_COUNT: usize = 5;

#[derive(Debug, Default, Clone)]
pub struct ValueRedactionStats {
    counts: [u64; VALUE_KIND_COUNT],
}

impl ValueRedactionStats {
    pub fn record(&mut self, kind: ValueKind) {
        self.counts[kind.index()] += 1;
    }

    pub fn count(&self, kind: ValueKind) -> u64 {
        self.counts[kind.index()]
    }

}

fn redacted_value(writer: &mut NonStreamingTraceWriter) -> ValueRecord {
    let ty = TraceWriter::ensure_type_id(writer, TypeKind::Raw, "Redacted");
    ValueRecord::Error {
        msg: REDACTED_SENTINEL.to_string(),
        type_id: ty,
    }
}

fn record_redaction(kind: ValueKind, candidate: &str, telemetry: Option<&mut ValueRedactionStats>) {
    if let Some(stats) = telemetry {
        stats.record(kind);
    }
    let metric = match kind {
        ValueKind::Arg => "filter_value_redacted.arg",
        ValueKind::Local => "filter_value_redacted.local",
        ValueKind::Global => "filter_value_redacted.global",
        ValueKind::Return => "filter_value_redacted.return",
        ValueKind::Attr => "filter_value_redacted.attr",
    };
    record_dropped_event(metric);
    log::debug!(
        "[RuntimeTracer] redacted {} '{}'",
        kind.label(),
        candidate
    );
}

fn encode_with_policy<'py>(
    py: Python<'py>,
    writer: &mut NonStreamingTraceWriter,
    value: &Bound<'py, PyAny>,
    policy: Option<&ValuePolicy>,
    kind: ValueKind,
    candidate: &str,
    telemetry: Option<&mut ValueRedactionStats>,
) -> ValueRecord {
    match policy.map(|p| p.decide(kind, candidate)) {
        Some(ValueAction::Deny) => {
            record_redaction(kind, candidate, telemetry);
            redacted_value(writer)
        }
        _ => encode_value(py, writer, value),
    }
}

/// Capture Python call arguments for the provided code object and encode them
/// using the runtime tracer writer.
pub fn capture_call_arguments<'py>(
    py: Python<'py>,
    writer: &mut NonStreamingTraceWriter,
    code: &CodeObjectWrapper,
    policy: Option<&ValuePolicy>,
    mut telemetry: Option<&mut ValueRedactionStats>,
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
        let value = locals.get_item(name)?.ok_or_else(|| {
            ffi::map_recorder_error(usage!(
                ErrorCode::MissingPositionalArgument,
                "missing positional arg '{name}'"
            ))
        })?;
        let encoded =
            encode_with_policy(py, writer, &value, policy, ValueKind::Arg, name, telemetry.as_deref_mut());
        args.push(TraceWriter::arg(writer, name, encoded));
        idx += 1;
    }

    if (flags & CO_VARARGS) != 0 && idx < varnames.len() {
        let name = &varnames[idx];
        if let Some(value) = locals.get_item(name)? {
            let encoded = encode_with_policy(
                py,
                writer,
                &value,
                policy,
                ValueKind::Arg,
                name,
                telemetry.as_deref_mut(),
            );
            args.push(TraceWriter::arg(writer, name, encoded));
        }
        idx += 1;
    }

    let kwonly_take = std::cmp::min(kwonly, varnames.len().saturating_sub(idx));
    for name in varnames.iter().skip(idx).take(kwonly_take) {
        let value = locals.get_item(name)?.ok_or_else(|| {
            ffi::map_recorder_error(usage!(
                ErrorCode::MissingKeywordArgument,
                "missing kw-only arg '{name}'"
            ))
        })?;
        let encoded = encode_with_policy(
            py,
            writer,
            &value,
            policy,
            ValueKind::Arg,
            name,
            telemetry.as_deref_mut(),
        );
        args.push(TraceWriter::arg(writer, name, encoded));
    }
    idx = idx.saturating_add(kwonly_take);

    if (flags & CO_VARKEYWORDS) != 0 && idx < varnames.len() {
        let name = &varnames[idx];
        if let Some(value) = locals.get_item(name)? {
            let encoded = encode_with_policy(
                py,
                writer,
                &value,
                policy,
                ValueKind::Arg,
                name,
                telemetry.as_deref_mut(),
            );
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
    policy: Option<&ValuePolicy>,
    mut telemetry: Option<&mut ValueRedactionStats>,
) {
    for (key, value) in snapshot.locals().iter() {
        let name = match key.downcast::<PyString>() {
            Ok(pystr) => match pystr.to_str() {
                Ok(raw) => raw.to_owned(),
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        let encoded = encode_with_policy(
            py,
            writer,
            &value,
            policy,
            ValueKind::Local,
            &name,
            telemetry.as_deref_mut(),
        );
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
            let encoded = encode_with_policy(
                py,
                writer,
                &value,
                policy,
                ValueKind::Global,
                name,
                telemetry.as_deref_mut(),
            );
            TraceWriter::register_variable_with_full_value(writer, name, encoded);
            recorded.insert(name.to_owned());
        }
    }
}

/// Encode and record a return value for the active trace.
pub fn record_return_value(
    py: Python<'_>,
    writer: &mut NonStreamingTraceWriter,
    value: &Bound<'_, PyAny>,
    policy: Option<&ValuePolicy>,
    mut telemetry: Option<&mut ValueRedactionStats>,
    candidate: Option<&str>,
) {
    let name = candidate.unwrap_or("<return>");
    let encoded = encode_with_policy(
        py,
        writer,
        value,
        policy,
        ValueKind::Return,
        name,
        telemetry.as_deref_mut(),
    );
    TraceWriter::register_return(writer, encoded);
}
