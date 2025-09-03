use std::path::{Path, PathBuf};

use pyo3::prelude::*;
use pyo3::types::PyAny;

use runtime_tracing::{Line, TraceEventsFileFormat, TraceWriter, TypeKind, ValueRecord, NONE_VALUE};
use runtime_tracing::NonStreamingTraceWriter;

use crate::code_object::CodeObjectWrapper;
use crate::tracer::{events_union, EventSet, MonitoringEvents, Tracer};

// Logging is handled via the `log` crate macros (e.g., log::debug!).

/// Minimal runtime tracer that maps Python sys.monitoring events to
/// runtime_tracing writer operations.
pub struct RuntimeTracer {
    writer: NonStreamingTraceWriter,
    format: TraceEventsFileFormat,
    // Activation control: when set, events are ignored until we see
    // a code object whose filename matches this path. Once triggered,
    // tracing becomes active for the remainder of the session.
    activation_path: Option<PathBuf>,
    // Code object id that triggered activation, used to stop on return
    activation_code_id: Option<usize>,
    // Whether we've already completed a one-shot activation window
    activation_done: bool,
    started: bool,
}

impl RuntimeTracer {
    pub fn new(
        program: &str,
        args: &[String],
        format: TraceEventsFileFormat,
        activation_path: Option<&Path>,
    ) -> Self {
        let mut writer = NonStreamingTraceWriter::new(program, args);
        writer.set_format(format);
        let activation_path = activation_path.map(|p| std::path::absolute(p).unwrap());
        // If activation path is specified, start in paused mode; otherwise start immediately.
        let started = activation_path.is_none();
        Self {
            writer,
            format,
            activation_path,
            activation_code_id: None,
            activation_done: false,
            started,
        }
    }

    /// Configure output files and write initial metadata records.
    pub fn begin(&mut self, meta_path: &Path, paths_path: &Path, events_path: &Path, start_path: &Path, start_line: u32) -> PyResult<()> {
        TraceWriter::begin_writing_trace_metadata(&mut self.writer, meta_path).map_err(to_py_err)?;
        TraceWriter::begin_writing_trace_paths(&mut self.writer, paths_path).map_err(to_py_err)?;
        TraceWriter::begin_writing_trace_events(&mut self.writer, events_path).map_err(to_py_err)?;
        TraceWriter::start(&mut self.writer, start_path, Line(start_line as i64));
        Ok(())
    }

    /// Return true when tracing is active; may become true on first event
    /// from the activation file if configured.
    fn ensure_started<'py>(&mut self, py: Python<'py>, code: &CodeObjectWrapper) {
        if self.started || self.activation_done { return; }
        if let Some(activation) = &self.activation_path {
            if let Ok(filename) = code.filename(py) {
                let f = Path::new(filename);
                //NOTE(Tzanko): We expect that code.filename contains an absolute path. If it turns out that this is sometimes not the case
                //we will investigate. For we won't do additional conversions here.
                // If there are issues the fool-proof solution is to use fs::canonicalize which needs to do syscalls
                if f == activation {
                    self.started = true;
                    self.activation_code_id = Some(code.id());
                    log::debug!("[RuntimeTracer] activated on enter: {}", activation.display());
                }
            }
        }
    }

    fn encode_value<'py>(&mut self, _py: Python<'py>, v: &Bound<'py, PyAny>) -> ValueRecord {
        // None
        if v.is_none() {
            return NONE_VALUE;
        }
        // bool must be checked before int in Python
        if let Ok(b) = v.extract::<bool>() {
            let ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::Bool, "Bool");
            return ValueRecord::Bool { b, type_id: ty };
        }
        if let Ok(i) = v.extract::<i64>() {
            let ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::Int, "Int");
            return ValueRecord::Int { i, type_id: ty };
        }
        if let Ok(s) = v.extract::<String>() {
            let ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::String, "String");
            return ValueRecord::String { text: s, type_id: ty };
        }

        // Fallback to Raw string representation
        let ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::Raw, "Object");
        match v.str() {
            Ok(s) => ValueRecord::Raw { r: s.to_string_lossy().into_owned(), type_id: ty },
            Err(_) => ValueRecord::Error { msg: "<unrepr>".to_string(), type_id: ty },
        }
    }

    fn ensure_function_id(&mut self, py: Python<'_>, code: &CodeObjectWrapper) -> PyResult<runtime_tracing::FunctionId> {
        //TODO AI! current runtime_tracer logic expects that `name` is unique and is used as a key for the function.
        //This is wrong. We need to write a test that exposes this issue
        let name = code.qualname(py)?;
        let filename = code.filename(py)?;
        let first_line = code.first_line(py)?;
        Ok(TraceWriter::ensure_function_id(&mut self.writer, name, Path::new(filename), Line(first_line as i64)))
    }
}

fn to_py_err(e: Box<dyn std::error::Error>) -> pyo3::PyErr {
    pyo3::exceptions::PyRuntimeError::new_err(e.to_string())
}

impl Tracer for RuntimeTracer {
    fn interest(&self, events: &MonitoringEvents) -> EventSet {
        // Minimal set: function start, step lines, and returns
        events_union(&[events.PY_START, events.LINE, events.PY_RETURN])
    }

    fn on_py_start(&mut self, py: Python<'_>, code: &CodeObjectWrapper, _offset: i32) {
        // Activate lazily if configured; ignore until then
        self.ensure_started(py, code);
        if !self.started { return; }
        // Trace event entry
        match (code.filename(py), code.qualname(py)) {
            (Ok(fname), Ok(qname)) => {
                log::debug!("[RuntimeTracer] on_py_start: {} ({})", qname, fname)
            }
            _ => log::debug!("[RuntimeTracer] on_py_start"),
        }
        if let Ok(fid) = self.ensure_function_id(py, code) {
            TraceWriter::register_call(&mut self.writer, fid, Vec::new());
        }
    }

    fn on_line(&mut self, py: Python<'_>, code: &CodeObjectWrapper, lineno: u32) {
        // Activate lazily if configured; ignore until then
        self.ensure_started(py, code);
        if !self.started { return; }
        // Trace event entry
        if let Ok(fname) = code.filename(py) {
            log::debug!("[RuntimeTracer] on_line: {}:{}", fname, lineno);
        } else {
            log::debug!("[RuntimeTracer] on_line: <unknown>:{}", lineno);
        }
        if let Ok(filename) = code.filename(py) {
            TraceWriter::register_step(&mut self.writer, Path::new(filename), Line(lineno as i64));
        }
    }

    fn on_py_return(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
        retval: &Bound<'_, PyAny>,
    ) {
        // Activate lazily if configured; ignore until then
        self.ensure_started(py, code);
        if !self.started { return; }
        // Trace event entry
        match (code.filename(py), code.qualname(py)) {
            (Ok(fname), Ok(qname)) => log::debug!("[RuntimeTracer] on_py_return: {} ({})", qname, fname),
            _ => log::debug!("[RuntimeTracer] on_py_return"),
        }
        // Determine whether this is the activation owner's return
        let is_activation_return = self.activation_code_id.map(|id| id == code.id()).unwrap_or(false);
        
        let val = self.encode_value(py, retval);
        TraceWriter::register_return(&mut self.writer, val);
        if is_activation_return {
            self.started = false;
            self.activation_done = true;
            log::debug!("[RuntimeTracer] deactivated on activation return");
        }
    }

    fn flush(&mut self, _py: Python<'_>) -> PyResult<()> {
        // Trace event entry
        log::debug!("[RuntimeTracer] flush");
        // For non-streaming formats we can update the events file.
        match self.format {
            TraceEventsFileFormat::Json | TraceEventsFileFormat::BinaryV0 => {
                TraceWriter::finish_writing_trace_events(&mut self.writer).map_err(to_py_err)?;
            }
            TraceEventsFileFormat::Binary => {
                // Streaming writer: no partial flush to avoid closing the stream.
            }
        }
        Ok(())
    }

    fn finish(&mut self, _py: Python<'_>) -> PyResult<()> {
        // Trace event entry
        log::debug!("[RuntimeTracer] finish");
        TraceWriter::finish_writing_trace_metadata(&mut self.writer).map_err(to_py_err)?;
        TraceWriter::finish_writing_trace_paths(&mut self.writer).map_err(to_py_err)?;
        TraceWriter::finish_writing_trace_events(&mut self.writer).map_err(to_py_err)?;
        Ok(())
    }
}
