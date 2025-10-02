//! Runtime tracer facade translating sys.monitoring callbacks into `runtime_tracing` records.

mod activation;
mod frame_inspector;
mod output_paths;
mod value_encoder;

pub use output_paths::TraceOutputPaths;

use activation::ActivationController;
use frame_inspector::capture_frame;
use value_encoder::encode_value;

use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::path::{Path, PathBuf};

use pyo3::prelude::*;
use pyo3::types::PyAny;
use pyo3::{ffi, PyErr};

use runtime_tracing::NonStreamingTraceWriter;
use runtime_tracing::{Line, TraceEventsFileFormat, TraceWriter};

use crate::code_object::CodeObjectWrapper;
use crate::monitoring::{
    events_union, CallbackOutcome, CallbackResult, EventSet, MonitoringEvents, Tracer,
};

extern "C" {
    fn PyFrame_GetLocals(frame: *mut ffi::PyFrameObject) -> *mut ffi::PyObject;
}

// Logging is handled via the `log` crate macros (e.g., log::debug!).

/// Minimal runtime tracer that maps Python sys.monitoring events to
/// runtime_tracing writer operations.
pub struct RuntimeTracer {
    writer: NonStreamingTraceWriter,
    format: TraceEventsFileFormat,
    activation: ActivationController,
    program_path: PathBuf,
    ignored_code_ids: HashSet<usize>,
    function_ids: HashMap<usize, runtime_tracing::FunctionId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShouldTrace {
    Trace,
    SkipAndDisable,
}

fn is_real_filename(filename: &str) -> bool {
    let trimmed = filename.trim();
    !(trimmed.starts_with('<') && trimmed.ends_with('>'))
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
        let activation = ActivationController::new(activation_path);
        let program_path = PathBuf::from(program);
        Self {
            writer,
            format,
            activation,
            program_path,
            ignored_code_ids: HashSet::new(),
            function_ids: HashMap::new(),
        }
    }

    /// Configure output files and write initial metadata records.
    pub fn begin(&mut self, outputs: &TraceOutputPaths, start_line: u32) -> PyResult<()> {
        let start_path = self.activation.start_path(&self.program_path);
        log::debug!("{}", start_path.display());
        outputs
            .configure_writer(&mut self.writer, start_path, start_line)
            .map_err(to_py_err)
    }

    fn ensure_function_id(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> PyResult<runtime_tracing::FunctionId> {
        match self.function_ids.entry(code.id()) {
            Entry::Occupied(entry) => Ok(*entry.get()),
            Entry::Vacant(slot) => {
                let name = code.qualname(py)?;
                let filename = code.filename(py)?;
                let first_line = code.first_line(py)?;
                let function_id = TraceWriter::ensure_function_id(
                    &mut self.writer,
                    name,
                    Path::new(filename),
                    Line(first_line as i64),
                );
                Ok(*slot.insert(function_id))
            }
        }
    }

    fn should_trace_code(&mut self, py: Python<'_>, code: &CodeObjectWrapper) -> ShouldTrace {
        let code_id = code.id();
        if self.ignored_code_ids.contains(&code_id) {
            return ShouldTrace::SkipAndDisable;
        }
        let filename = code
            .filename(py)
            .expect("RuntimeTracer::should_trace_code failed to resolve filename");
        if is_real_filename(filename) {
            ShouldTrace::Trace
        } else {
            self.ignored_code_ids.insert(code_id);
            ShouldTrace::SkipAndDisable
        }
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

    fn on_py_start(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
    ) -> CallbackResult {
        let is_active = self.activation.should_process_event(py, code);
        if matches!(
            self.should_trace_code(py, code),
            ShouldTrace::SkipAndDisable
        ) {
            return Ok(CallbackOutcome::DisableLocation);
        }
        if !is_active {
            return Ok(CallbackOutcome::Continue);
        }

        match (code.filename(py), code.qualname(py)) {
            (Ok(fname), Ok(qname)) => {
                log::debug!("[RuntimeTracer] on_py_start: {} ({})", qname, fname)
            }
            _ => log::debug!("[RuntimeTracer] on_py_start"),
        }

        if let Ok(fid) = self.ensure_function_id(py, code) {
            let mut args: Vec<runtime_tracing::FullValueRecord> = Vec::new();
            let frame_and_args = (|| -> PyResult<()> {
                let frame_ptr = unsafe { ffi::PyEval_GetFrame() };
                if frame_ptr.is_null() {
                    return Err(pyo3::exceptions::PyRuntimeError::new_err(
                        "on_py_start: null frame",
                    ));
                }
                unsafe {
                    ffi::Py_XINCREF(frame_ptr.cast());
                }

                unsafe {
                    if ffi::PyFrame_FastToLocalsWithError(frame_ptr) < 0 {
                        ffi::Py_DECREF(frame_ptr.cast());
                        let err = PyErr::fetch(py);
                        return Err(err);
                    }
                }

                let locals_raw = unsafe { PyFrame_GetLocals(frame_ptr) };
                if locals_raw.is_null() {
                    unsafe {
                        ffi::Py_DECREF(frame_ptr.cast());
                    }
                    return Err(pyo3::exceptions::PyRuntimeError::new_err(
                        "on_py_start: PyFrame_GetLocals returned null",
                    ));
                }
                let locals = unsafe { Bound::<PyAny>::from_owned_ptr(py, locals_raw) };

                let argcount = code.arg_count(py)? as usize;
                let _posonly: usize = code.as_bound(py).getattr("co_posonlyargcount")?.extract()?;
                let kwonly: usize = code.as_bound(py).getattr("co_kwonlyargcount")?.extract()?;
                let flags = code.flags(py)?;
                const CO_VARARGS: u32 = 0x04;
                const CO_VARKEYWORDS: u32 = 0x08;

                let varnames_obj = code.as_bound(py).getattr("co_varnames")?;
                let varnames: Vec<String> = varnames_obj.extract()?;

                let mut idx = 0usize;
                let take_n = std::cmp::min(argcount, varnames.len());
                for name in varnames.iter().take(take_n) {
                    match locals.get_item(name) {
                        Ok(val) => {
                            let vrec = encode_value(py, &mut self.writer, &val);
                            args.push(TraceWriter::arg(&mut self.writer, name, vrec));
                        }
                        Err(e) => {
                            panic!("Error {:?}", e)
                        }
                    }
                    idx += 1;
                }

                if (flags & CO_VARARGS) != 0 && idx < varnames.len() {
                    let name = &varnames[idx];
                    if let Ok(val) = locals.get_item(name) {
                        let vrec = encode_value(py, &mut self.writer, &val);
                        args.push(TraceWriter::arg(&mut self.writer, name, vrec));
                    }
                    idx += 1;
                }

                let kwonly_take = std::cmp::min(kwonly, varnames.len().saturating_sub(idx));
                for name in varnames.iter().skip(idx).take(kwonly_take) {
                    match locals.get_item(name) {
                        Ok(val) => {
                            let vrec = encode_value(py, &mut self.writer, &val);
                            args.push(TraceWriter::arg(&mut self.writer, name, vrec));
                        }
                        Err(e) => {
                            panic!("Error {:?}", e)
                        }
                    }
                }
                idx = idx.saturating_add(kwonly_take);

                if (flags & CO_VARKEYWORDS) != 0 && idx < varnames.len() {
                    let name = &varnames[idx];
                    if let Ok(val) = locals.get_item(name) {
                        let vrec = encode_value(py, &mut self.writer, &val);
                        args.push(TraceWriter::arg(&mut self.writer, name, vrec));
                    }
                }
                unsafe {
                    ffi::Py_DECREF(frame_ptr.cast());
                }
                Ok(())
            })();

            if let Err(e) = frame_and_args {
                let message = format!("on_py_start: failed to capture args: {}", e);
                log::error!("{message}");
                return Err(pyo3::exceptions::PyRuntimeError::new_err(message));
            }

            TraceWriter::register_call(&mut self.writer, fid, args);
        }

        Ok(CallbackOutcome::Continue)
    }

    fn on_line(&mut self, py: Python<'_>, code: &CodeObjectWrapper, lineno: u32) -> CallbackResult {
        let is_active = self.activation.should_process_event(py, code);
        if matches!(
            self.should_trace_code(py, code),
            ShouldTrace::SkipAndDisable
        ) {
            return Ok(CallbackOutcome::DisableLocation);
        }
        if !is_active {
            return Ok(CallbackOutcome::Continue);
        }

        if let Ok(fname) = code.filename(py) {
            log::debug!("[RuntimeTracer] on_line: {}:{}", fname, lineno);
        } else {
            log::debug!("[RuntimeTracer] on_line: <unknown>:{}", lineno);
        }

        if let Ok(filename) = code.filename(py) {
            TraceWriter::register_step(&mut self.writer, Path::new(filename), Line(lineno as i64));
        }

        let snapshot = capture_frame(py, code)?;

        let mut recorded: HashSet<String> = HashSet::new();

        for (key, value) in snapshot.locals().iter() {
            let name: String = key.extract().expect("Local name was not a string");
            let encoded = encode_value(py, &mut self.writer, &value);
            TraceWriter::register_variable_with_full_value(&mut self.writer, &name, encoded);
            recorded.insert(name);
        }

        if !snapshot.locals_is_globals() {
            if let Some(globals_dict) = snapshot.globals() {
                for (key, value) in globals_dict.iter() {
                    let name: String = key.extract().expect("Global name was not a string");
                    if name == "__builtins__" || recorded.contains(&name) {
                        continue;
                    }
                    let encoded = encode_value(py, &mut self.writer, &value);
                    TraceWriter::register_variable_with_full_value(
                        &mut self.writer,
                        &name,
                        encoded,
                    );
                    recorded.insert(name);
                }
            }
        }

        Ok(CallbackOutcome::Continue)
    }

    fn on_py_return(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
        _offset: i32,
        retval: &Bound<'_, PyAny>,
    ) -> CallbackResult {
        let is_active = self.activation.should_process_event(py, code);
        if matches!(
            self.should_trace_code(py, code),
            ShouldTrace::SkipAndDisable
        ) {
            return Ok(CallbackOutcome::DisableLocation);
        }
        if !is_active {
            return Ok(CallbackOutcome::Continue);
        }

        match (code.filename(py), code.qualname(py)) {
            (Ok(fname), Ok(qname)) => {
                log::debug!("[RuntimeTracer] on_py_return: {} ({})", qname, fname)
            }
            _ => log::debug!("[RuntimeTracer] on_py_return"),
        }

        let val = encode_value(py, &mut self.writer, retval);
        TraceWriter::register_return(&mut self.writer, val);
        if self.activation.handle_return_event(code.id()) {
            log::debug!("[RuntimeTracer] deactivated on activation return");
        }

        Ok(CallbackOutcome::Continue)
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
        self.ignored_code_ids.clear();
        Ok(())
    }

    fn finish(&mut self, _py: Python<'_>) -> PyResult<()> {
        // Trace event entry
        log::debug!("[RuntimeTracer] finish");
        TraceWriter::finish_writing_trace_metadata(&mut self.writer).map_err(to_py_err)?;
        TraceWriter::finish_writing_trace_paths(&mut self.writer).map_err(to_py_err)?;
        TraceWriter::finish_writing_trace_events(&mut self.writer).map_err(to_py_err)?;
        self.ignored_code_ids.clear();
        self.function_ids.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::CallbackOutcome;
    use pyo3::types::{PyCode, PyModule};
    use pyo3::wrap_pyfunction;
    use runtime_tracing::{FullValueRecord, TraceLowLevelEvent, ValueRecord};
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::ffi::CString;

    thread_local! {
        static ACTIVE_TRACER: Cell<*mut RuntimeTracer> = Cell::new(std::ptr::null_mut());
        static LAST_OUTCOME: Cell<Option<CallbackOutcome>> = Cell::new(None);
    }

    struct ScopedTracer;

    impl ScopedTracer {
        fn new(tracer: &mut RuntimeTracer) -> Self {
            let ptr = tracer as *mut _;
            ACTIVE_TRACER.with(|cell| cell.set(ptr));
            ScopedTracer
        }
    }

    impl Drop for ScopedTracer {
        fn drop(&mut self) {
            ACTIVE_TRACER.with(|cell| cell.set(std::ptr::null_mut()));
        }
    }

    fn last_outcome() -> Option<CallbackOutcome> {
        LAST_OUTCOME.with(|cell| cell.get())
    }

    #[test]
    fn detects_real_filenames() {
        assert!(is_real_filename("example.py"));
        assert!(is_real_filename(" /tmp/module.py "));
        assert!(is_real_filename("src/<tricky>.py"));
        assert!(!is_real_filename("<string>"));
        assert!(!is_real_filename("  <stdin>  "));
        assert!(!is_real_filename("<frozen importlib._bootstrap>"));
    }

    #[test]
    fn skips_synthetic_filename_events() {
        Python::with_gil(|py| {
            let mut tracer = RuntimeTracer::new("test.py", &[], TraceEventsFileFormat::Json, None);
            ensure_test_module(py);
            let script = format!("{PRELUDE}\nsnapshot()\n");
            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let script_c = CString::new(script).expect("script contains nul byte");
                py.run(script_c.as_c_str(), None, None)
                    .expect("execute synthetic script");
            }
            assert!(
                tracer.writer.events.is_empty(),
                "expected no events for synthetic filename"
            );
            assert_eq!(last_outcome(), Some(CallbackOutcome::DisableLocation));

            let compile_fn = py
                .import("builtins")
                .expect("import builtins")
                .getattr("compile")
                .expect("fetch compile");
            let binding = compile_fn
                .call1(("pass", "<string>", "exec"))
                .expect("compile code object");
            let code_obj = binding.downcast::<PyCode>().expect("downcast code object");
            let wrapper = CodeObjectWrapper::new(py, &code_obj);
            assert_eq!(
                tracer.should_trace_code(py, &wrapper),
                ShouldTrace::SkipAndDisable
            );
        });
    }

    #[test]
    fn traces_real_file_events() {
        let snapshots = run_traced_script("snapshot()\n");
        assert!(
            !snapshots.is_empty(),
            "expected snapshots for real file execution"
        );
        assert_eq!(last_outcome(), Some(CallbackOutcome::Continue));
    }

    #[test]
    fn callbacks_do_not_import_sys_monitoring() {
        let body = r#"
import builtins
_orig_import = builtins.__import__

def guard(name, *args, **kwargs):
    if name == "sys.monitoring":
        raise RuntimeError("callback imported sys.monitoring")
    return _orig_import(name, *args, **kwargs)

builtins.__import__ = guard
try:
    snapshot()
finally:
    builtins.__import__ = _orig_import
"#;
        let snapshots = run_traced_script(body);
        assert!(
            !snapshots.is_empty(),
            "expected snapshots when import guard active"
        );
        assert_eq!(last_outcome(), Some(CallbackOutcome::Continue));
    }

    #[pyfunction]
    fn capture_line(py: Python<'_>, code: Bound<'_, PyCode>, lineno: u32) -> PyResult<()> {
        ACTIVE_TRACER.with(|cell| -> PyResult<()> {
            let ptr = cell.get();
            if ptr.is_null() {
                panic!("No active RuntimeTracer for capture_line");
            }
            unsafe {
                let tracer = &mut *ptr;
                let wrapper = CodeObjectWrapper::new(py, &code);
                match tracer.on_line(py, &wrapper, lineno) {
                    Ok(outcome) => {
                        LAST_OUTCOME.with(|cell| cell.set(Some(outcome)));
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            }
        })?;
        Ok(())
    }

    const PRELUDE: &str = r#"
import inspect
from test_tracer import capture_line

def snapshot(line=None):
    frame = inspect.currentframe().f_back
    lineno = frame.f_lineno if line is None else line
    capture_line(frame.f_code, lineno)

def snap(value):
    frame = inspect.currentframe().f_back
    capture_line(frame.f_code, frame.f_lineno)
    return value
"#;

    #[derive(Debug, Clone, PartialEq)]
    enum SimpleValue {
        None,
        Bool(bool),
        Int(i64),
        String(String),
        Tuple(Vec<SimpleValue>),
        Sequence(Vec<SimpleValue>),
        Raw(String),
    }

    impl SimpleValue {
        fn from_value(value: &ValueRecord) -> Self {
            match value {
                ValueRecord::None { .. } => SimpleValue::None,
                ValueRecord::Bool { b, .. } => SimpleValue::Bool(*b),
                ValueRecord::Int { i, .. } => SimpleValue::Int(*i),
                ValueRecord::String { text, .. } => SimpleValue::String(text.clone()),
                ValueRecord::Tuple { elements, .. } => {
                    SimpleValue::Tuple(elements.iter().map(SimpleValue::from_value).collect())
                }
                ValueRecord::Sequence { elements, .. } => {
                    SimpleValue::Sequence(elements.iter().map(SimpleValue::from_value).collect())
                }
                ValueRecord::Raw { r, .. } => SimpleValue::Raw(r.clone()),
                ValueRecord::Error { msg, .. } => SimpleValue::Raw(msg.clone()),
                other => SimpleValue::Raw(format!("{other:?}")),
            }
        }
    }

    #[derive(Debug)]
    struct Snapshot {
        line: i64,
        vars: BTreeMap<String, SimpleValue>,
    }

    fn collect_snapshots(events: &[TraceLowLevelEvent]) -> Vec<Snapshot> {
        let mut names: Vec<String> = Vec::new();
        let mut snapshots: Vec<Snapshot> = Vec::new();
        let mut current: Option<Snapshot> = None;
        for event in events {
            match event {
                TraceLowLevelEvent::VariableName(name) => names.push(name.clone()),
                TraceLowLevelEvent::Step(step) => {
                    if let Some(snapshot) = current.take() {
                        snapshots.push(snapshot);
                    }
                    current = Some(Snapshot {
                        line: step.line.0,
                        vars: BTreeMap::new(),
                    });
                }
                TraceLowLevelEvent::Value(FullValueRecord { variable_id, value }) => {
                    if let Some(snapshot) = current.as_mut() {
                        let index = variable_id.0;
                        let name = names
                            .get(index)
                            .cloned()
                            .unwrap_or_else(|| panic!("Missing variable name for id {}", index));
                        snapshot.vars.insert(name, SimpleValue::from_value(value));
                    }
                }
                _ => {}
            }
        }
        if let Some(snapshot) = current.take() {
            snapshots.push(snapshot);
        }
        snapshots
    }

    fn ensure_test_module(py: Python<'_>) {
        let module = PyModule::new(py, "test_tracer").expect("create module");
        module
            .add_function(wrap_pyfunction!(capture_line, &module).expect("wrap capture_line"))
            .expect("add function");
        py.import("sys")
            .expect("import sys")
            .getattr("modules")
            .expect("modules attr")
            .set_item("test_tracer", module)
            .expect("insert module");
    }

    fn run_traced_script(body: &str) -> Vec<Snapshot> {
        Python::with_gil(|py| {
            let mut tracer = RuntimeTracer::new("test.py", &[], TraceEventsFileFormat::Json, None);
            ensure_test_module(py);
            let tmp = tempfile::tempdir().expect("create temp dir");
            let script_path = tmp.path().join("script.py");
            let script = format!("{PRELUDE}\n{body}");
            std::fs::write(&script_path, &script).expect("write script");
            {
                let _guard = ScopedTracer::new(&mut tracer);
                LAST_OUTCOME.with(|cell| cell.set(None));
                let run_code = format!(
                    "import runpy\nrunpy.run_path(r\"{}\")",
                    script_path.display()
                );
                let run_code_c = CString::new(run_code).expect("script contains nul byte");
                py.run(run_code_c.as_c_str(), None, None)
                    .expect("execute test script");
            }
            collect_snapshots(&tracer.writer.events)
        })
    }

    fn assert_var(snapshot: &Snapshot, name: &str, expected: SimpleValue) {
        let actual = snapshot
            .vars
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing at line {}", snapshot.line));
        assert_eq!(
            actual, &expected,
            "Unexpected value for {name} at line {}",
            snapshot.line
        );
    }

    fn find_snapshot_with_vars<'a>(snapshots: &'a [Snapshot], names: &[&str]) -> &'a Snapshot {
        snapshots
            .iter()
            .find(|snap| names.iter().all(|n| snap.vars.contains_key(*n)))
            .unwrap_or_else(|| panic!("No snapshot containing variables {:?}", names))
    }

    fn assert_no_variable(snapshots: &[Snapshot], name: &str) {
        if snapshots.iter().any(|snap| snap.vars.contains_key(name)) {
            panic!("Variable {name} unexpectedly captured");
        }
    }

    #[test]
    fn captures_simple_function_locals() {
        let snapshots = run_traced_script(
            r#"
def simple_function(x):
    snapshot()
    a = 1
    snapshot()
    b = a + x
    snapshot()
    return a, b

simple_function(5)
"#,
        );

        assert_var(&snapshots[0], "x", SimpleValue::Int(5));
        assert!(!snapshots[0].vars.contains_key("a"));
        assert_var(&snapshots[1], "a", SimpleValue::Int(1));
        assert_var(&snapshots[2], "b", SimpleValue::Int(6));
    }

    #[test]
    fn captures_closure_variables() {
        let snapshots = run_traced_script(
            r#"
def outer_func(x):
    snapshot()
    y = 1
    snapshot()
    def inner_func(z):
        nonlocal y
        snapshot()
        w = x + y + z
        snapshot()
        y = w
        snapshot()
        return w
    total = inner_func(5)
    snapshot()
    return y, total

result = outer_func(2)
"#,
        );

        let inner_entry = find_snapshot_with_vars(&snapshots, &["x", "y", "z"]);
        assert_var(inner_entry, "x", SimpleValue::Int(2));
        assert_var(inner_entry, "y", SimpleValue::Int(1));

        let w_snapshot = find_snapshot_with_vars(&snapshots, &["w", "x", "y", "z"]);
        assert_var(w_snapshot, "w", SimpleValue::Int(8));

        let outer_after = find_snapshot_with_vars(&snapshots, &["total", "y"]);
        assert_var(outer_after, "total", SimpleValue::Int(8));
        assert_var(outer_after, "y", SimpleValue::Int(8));
    }

    #[test]
    fn captures_globals() {
        let snapshots = run_traced_script(
            r#"
GLOBAL_VAL = 10
counter = 0
snapshot()

def global_test():
    snapshot()
    local_copy = GLOBAL_VAL
    snapshot()
    global counter
    counter += 1
    snapshot()
    return local_copy, counter

before = counter
snapshot()
result = global_test()
snapshot()
after = counter
snapshot()
"#,
        );

        let access_global = find_snapshot_with_vars(&snapshots, &["local_copy", "GLOBAL_VAL"]);
        assert_var(access_global, "GLOBAL_VAL", SimpleValue::Int(10));
        assert_var(access_global, "local_copy", SimpleValue::Int(10));

        let last_counter = snapshots
            .iter()
            .rev()
            .find(|snap| snap.vars.contains_key("counter"))
            .expect("Expected at least one counter snapshot");
        assert_var(last_counter, "counter", SimpleValue::Int(1));
    }

    #[test]
    fn captures_class_scope() {
        let snapshots = run_traced_script(
            r#"
CONSTANT = 42
snapshot()

class MetaCounter(type):
    count = 0
    snapshot()
    def __init__(cls, name, bases, attrs):
        snapshot()
        MetaCounter.count += 1
        super().__init__(name, bases, attrs)

class Sample(metaclass=MetaCounter):
    snapshot()
    a = 10
    snapshot()
    b = a + 5
    snapshot()
    print(a, b, CONSTANT)
    snapshot()
    def method(self):
        snapshot()
        return self.a + self.b

instance = Sample()
snapshot()
instances = MetaCounter.count
snapshot()
_ = instance.method()
snapshot()
"#,
        );

        let meta_init = find_snapshot_with_vars(&snapshots, &["cls", "name", "attrs"]);
        assert_var(meta_init, "name", SimpleValue::String("Sample".to_string()));

        let class_body = find_snapshot_with_vars(&snapshots, &["a", "b"]);
        assert_var(class_body, "a", SimpleValue::Int(10));
        assert_var(class_body, "b", SimpleValue::Int(15));

        let method_snapshot = find_snapshot_with_vars(&snapshots, &["self"]);
        assert!(method_snapshot.vars.contains_key("self"));
    }

    #[test]
    fn captures_lambda_and_comprehensions() {
        let snapshots = run_traced_script(
            r#"
factor = 2
snapshot()
double = lambda y: snap(y * factor)
snapshot()
lambda_value = double(5)
snapshot()
squares = [snap(n ** 2) for n in range(3)]
snapshot()
scaled_set = {snap(n * factor) for n in range(3)}
snapshot()
mapping = {n: snap(n * factor) for n in range(3)}
snapshot()
gen_exp = (snap(n * factor) for n in range(3))
snapshot()
result_list = list(gen_exp)
snapshot()
"#,
        );

        let lambda_snapshot = find_snapshot_with_vars(&snapshots, &["y", "factor"]);
        assert_var(lambda_snapshot, "y", SimpleValue::Int(5));
        assert_var(lambda_snapshot, "factor", SimpleValue::Int(2));

        let list_comp = find_snapshot_with_vars(&snapshots, &["n", "factor"]);
        assert!(matches!(list_comp.vars.get("n"), Some(SimpleValue::Int(_))));

        let result_snapshot = find_snapshot_with_vars(&snapshots, &["result_list"]);
        assert!(matches!(
            result_snapshot.vars.get("result_list"),
            Some(SimpleValue::Sequence(_))
        ));
    }

    #[test]
    fn captures_generators_and_coroutines() {
        let snapshots = run_traced_script(
            r#"
import asyncio
snapshot()


def counter_gen(n):
    snapshot()
    total = 0
    for i in range(n):
        total += i
        snapshot()
        yield total
    snapshot()
    return total

async def async_sum(data):
    snapshot()
    total = 0
    for x in data:
        total += x
        snapshot()
        await asyncio.sleep(0)
    snapshot()
    return total

gen = counter_gen(3)
gen_results = list(gen)
snapshot()
coroutine_result = asyncio.run(async_sum([1, 2, 3]))
snapshot()
"#,
        );

        let generator_step = find_snapshot_with_vars(&snapshots, &["i", "total"]);
        assert!(matches!(
            generator_step.vars.get("i"),
            Some(SimpleValue::Int(_))
        ));

        let coroutine_steps: Vec<&Snapshot> = snapshots
            .iter()
            .filter(|snap| snap.vars.contains_key("x"))
            .collect();
        assert!(!coroutine_steps.is_empty());
        let final_coroutine_step = coroutine_steps.last().unwrap();
        assert_var(final_coroutine_step, "total", SimpleValue::Int(6));

        let coroutine_result_snapshot = find_snapshot_with_vars(&snapshots, &["coroutine_result"]);
        assert!(coroutine_result_snapshot
            .vars
            .contains_key("coroutine_result"));
    }

    #[test]
    fn captures_exception_and_with_blocks() {
        let snapshots = run_traced_script(
            r#"
import io
__file__ = "test_script.py"

def exception_and_with_demo(x):
    snapshot()
    try:
        inv = 10 / x
        snapshot()
    except ZeroDivisionError as e:
        snapshot()
        error_msg = f"Error: {e}"
        snapshot()
    else:
        snapshot()
        inv += 1
        snapshot()
    finally:
        snapshot()
        final_flag = True
        snapshot()
    with io.StringIO("dummy line") as f:
        snapshot()
        first_line = f.readline()
        snapshot()
    snapshot()
    return locals()

result1 = exception_and_with_demo(0)
snapshot()
result2 = exception_and_with_demo(5)
snapshot()
"#,
        );

        let except_snapshot = find_snapshot_with_vars(&snapshots, &["e", "error_msg"]);
        assert!(matches!(
            except_snapshot.vars.get("error_msg"),
            Some(SimpleValue::String(_))
        ));

        let finally_snapshot = find_snapshot_with_vars(&snapshots, &["final_flag"]);
        assert_var(finally_snapshot, "final_flag", SimpleValue::Bool(true));

        let with_snapshot = find_snapshot_with_vars(&snapshots, &["f", "first_line"]);
        assert!(with_snapshot.vars.contains_key("first_line"));
    }

    #[test]
    fn captures_decorators() {
        let snapshots = run_traced_script(
            r#"
setting = "Hello"
snapshot()


def my_decorator(func):
    snapshot()
    def wrapper(*args, **kwargs):
        snapshot()
        return func(*args, **kwargs)
    return wrapper

@my_decorator
def greet(name):
    snapshot()
    message = f"Hi, {name}"
    snapshot()
    return message

output = greet("World")
snapshot()
"#,
        );

        let decorator_snapshot = find_snapshot_with_vars(&snapshots, &["func", "setting"]);
        assert!(decorator_snapshot.vars.contains_key("func"));

        let wrapper_snapshot = find_snapshot_with_vars(&snapshots, &["args", "kwargs", "setting"]);
        assert!(wrapper_snapshot.vars.contains_key("args"));

        let greet_snapshot = find_snapshot_with_vars(&snapshots, &["name", "message"]);
        assert_var(
            greet_snapshot,
            "name",
            SimpleValue::String("World".to_string()),
        );
    }

    #[test]
    fn captures_dynamic_execution() {
        let snapshots = run_traced_script(
            r#"
expr_code = "dynamic_var = 99"
snapshot()
exec(expr_code)
snapshot()
check = dynamic_var + 1
snapshot()

def eval_test():
    snapshot()
    value = 10
    formula = "value * 2"
    snapshot()
    result = eval(formula)
    snapshot()
    return result

out = eval_test()
snapshot()
"#,
        );

        let exec_snapshot = find_snapshot_with_vars(&snapshots, &["dynamic_var"]);
        assert_var(exec_snapshot, "dynamic_var", SimpleValue::Int(99));

        let eval_snapshot = find_snapshot_with_vars(&snapshots, &["value", "formula"]);
        assert_var(eval_snapshot, "value", SimpleValue::Int(10));
    }

    #[test]
    fn captures_imports() {
        let snapshots = run_traced_script(
            r#"
import math
snapshot()

def import_test():
    snapshot()
    import os
    snapshot()
    constant = math.pi
    snapshot()
    cwd = os.getcwd()
    snapshot()
    return constant, cwd

val, path = import_test()
snapshot()
"#,
        );

        let global_import = find_snapshot_with_vars(&snapshots, &["math"]);
        assert!(matches!(
            global_import.vars.get("math"),
            Some(SimpleValue::Raw(_))
        ));

        let local_import = find_snapshot_with_vars(&snapshots, &["os", "constant"]);
        assert!(local_import.vars.contains_key("os"));
    }

    #[test]
    fn builtins_not_recorded() {
        let snapshots = run_traced_script(
            r#"
def builtins_test(seq):
    snapshot()
    n = len(seq)
    snapshot()
    m = max(seq)
    snapshot()
    return n, m

result = builtins_test([5, 3, 7])
snapshot()
"#,
        );

        let len_snapshot = find_snapshot_with_vars(&snapshots, &["n"]);
        assert_var(len_snapshot, "n", SimpleValue::Int(3));
        assert_no_variable(&snapshots, "len");
    }
}
