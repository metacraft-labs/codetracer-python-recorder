use std::path::{Path, PathBuf};

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyList, PyTuple, PyDict};

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

    /// Encode a Python value into a `ValueRecord` used by the trace writer.
    ///
    /// Canonical rules:
    /// - `None` -> `NONE_VALUE`
    /// - `bool` -> `Bool`
    /// - `int`  -> `Int`
    /// - `str`  -> `String` (canonical for text; do not fall back to Raw)
    /// - common containers:
    ///   - Python `tuple` -> `Tuple` with encoded elements
    ///   - Python `list`  -> `Sequence` with encoded elements (not a slice)
    /// - any other type -> textual `Raw` via `__str__` best-effort
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
        // Strings are encoded canonically as `String` to ensure stable tests
        // and downstream processing. Falling back to `Raw` for `str` is
        // not allowed.
        if let Ok(s) = v.extract::<String>() {
            let ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::String, "String");
            return ValueRecord::String { text: s, type_id: ty };
        }

        // Python tuple -> ValueRecord::Tuple with recursively-encoded elements
        if let Ok(t) = v.downcast::<PyTuple>() {
            let mut elements: Vec<ValueRecord> = Vec::with_capacity(t.len());
            for item in t.iter() {
                // item: Bound<PyAny>
                elements.push(self.encode_value(_py, &item));
            }
            let ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::Tuple, "Tuple");
            return ValueRecord::Tuple { elements, type_id: ty };
        }

        // Python list -> ValueRecord::Sequence with recursively-encoded elements
        if let Ok(l) = v.downcast::<PyList>() {
            let mut elements: Vec<ValueRecord> = Vec::with_capacity(l.len());
            for item in l.iter() {
                elements.push(self.encode_value(_py, &item));
            }
            let ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::Seq, "List");
            return ValueRecord::Sequence { elements, is_slice: false, type_id: ty };
        }

        // Python dict -> represent as a Sequence of (key, value) Tuples.
        // Keys are expected to be strings for kwargs; for non-str keys we
        // fall back to best-effort encoding of the key.
        if let Ok(d) = v.downcast::<PyDict>() {
            let seq_ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::Seq, "Dict");
            let tuple_ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::Tuple, "Tuple");
            let str_ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::String, "String");
            let mut elements: Vec<ValueRecord> = Vec::with_capacity(d.len());
            let items = d.items();
            for pair in items.iter() {
                if let Ok(t) = pair.downcast::<PyTuple>() {
                    if t.len() == 2 {
                        let key_obj = t.get_item(0).unwrap();
                        let val_obj = t.get_item(1).unwrap();
                        let key_rec = if let Ok(s) = key_obj.extract::<String>() {
                            ValueRecord::String { text: s, type_id: str_ty }
                        } else {
                            self.encode_value(_py, &key_obj)
                        };
                        let val_rec = self.encode_value(_py, &val_obj);
                        let pair_rec = ValueRecord::Tuple { elements: vec![key_rec, val_rec], type_id: tuple_ty };
                        elements.push(pair_rec);
                    }
                }
            }
            return ValueRecord::Sequence { elements, is_slice: false, type_id: seq_ty };
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

    fn on_py_start(&mut self, py: Python<'_>, code: &CodeObjectWrapper, _offset: i32) -> PyResult<()> {
        // Activate lazily if configured; ignore until then
        self.ensure_started(py, code);
        if !self.started { return Ok(()); }
        // Trace event entry
        match (code.filename(py), code.qualname(py)) {
            (Ok(fname), Ok(qname)) => {
                log::debug!("[RuntimeTracer] on_py_start: {} ({})", qname, fname)
            }
            _ => log::debug!("[RuntimeTracer] on_py_start"),
        }
        if let Ok(fid) = self.ensure_function_id(py, code) {
            // Attempt to capture function arguments from the current frame.
            // Fail fast on any error per source-code rules.
            let mut args: Vec<runtime_tracing::FullValueRecord> = Vec::new();
            let frame_and_args = (|| -> PyResult<()> {
                // Current Python frame where the function just started executing
                let sys = py.import("sys")?;
                let frame = sys.getattr("_getframe")?.call1((0,))?;
                let locals = frame.getattr("f_locals")?;

                // Argument names come from co_varnames in the order defined by CPython:
                // [positional (pos-only + pos-or-kw)] [+ varargs] [+ kw-only] [+ kwargs]
                // In CPython 3.8+ semantics, `co_argcount` is the TOTAL number of positional
                // parameters (including positional-only and pos-or-keyword). Use it directly
                // for the positional slice; `co_posonlyargcount` is only needed if we want to
                // distinguish the two groups, which we do not here.
                let argcount = code.arg_count(py)? as usize; // total positional (pos-only + pos-or-kw)
                let posonly: usize = code
                    .as_bound(py)
                    .getattr("co_posonlyargcount")?
                    .extract()?;
                let kwonly: usize = code
                    .as_bound(py)
                    .getattr("co_kwonlyargcount")?
                    .extract()?;

                let flags = code.flags(py)?;
                const CO_VARARGS: u32 = 0x04;
                const CO_VARKEYWORDS: u32 = 0x08;

                let varnames_obj = code.as_bound(py).getattr("co_varnames")?;
                let varnames: Vec<String> = varnames_obj.extract()?;

                // 1) Positional parameters (pos-only + pos-or-kw)
                let mut idx = 0usize;
                // `argcount` already includes positional-only parameters
                let take_n = std::cmp::min(argcount, varnames.len());
                for name in varnames.iter().take(take_n) {
                    match locals.get_item(name) {
                        Ok(val) => {
                            let vrec = self.encode_value(py, &val);
                            args.push(TraceWriter::arg(&mut self.writer, name, vrec));
                        }
                        Err(_) => {}
                    }
                    idx += 1;
                }

                // 2) Varargs (*args)
                if (flags & CO_VARARGS) != 0 && idx < varnames.len() {
                    let name = &varnames[idx];
                    if let Ok(val) = locals.get_item(name) {
                        let vrec = self.encode_value(py, &val);
                        args.push(TraceWriter::arg(&mut self.writer, name, vrec));
                    }
                    idx += 1;
                }

                // 3) Keyword-only parameters
                let kwonly_take = std::cmp::min(kwonly, varnames.len().saturating_sub(idx));
                for name in varnames.iter().skip(idx).take(kwonly_take) {
                    match locals.get_item(name) {
                        Ok(val) => {
                            let vrec = self.encode_value(py, &val);
                            args.push(TraceWriter::arg(&mut self.writer, name, vrec));
                        }
                        Err(_) => {}
                    }
                }
                idx = idx.saturating_add(kwonly_take);

                // 4) Kwargs (**kwargs)
                if (flags & CO_VARKEYWORDS) != 0 && idx < varnames.len() {
                    let name = &varnames[idx];
                    if let Ok(val) = locals.get_item(name) {
                        let vrec = self.encode_value(py, &val);
                        args.push(TraceWriter::arg(&mut self.writer, name, vrec));
                    }
                }
                Ok(())
            })();
            if let Err(e) = frame_and_args {
                // Raise a clear error; do not silently continue with empty args.
                let rete =Err(pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "on_py_start: failed to capture args: {}",
                    e
                )));
                log::debug!("error {:?}", rete);
                return rete;
            }

            TraceWriter::register_call(&mut self.writer, fid, args);
        }
        Ok(())
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
