use std::collections::HashSet;
use std::path::{Path, PathBuf};

use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyFrame, PyMapping};
use pyo3::Py;

use runtime_tracing::NonStreamingTraceWriter;
use runtime_tracing::{
    Line, TraceEventsFileFormat, TraceWriter, TypeKind, ValueRecord, NONE_VALUE,
};

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
    pub fn begin(
        &mut self,
        meta_path: &Path,
        paths_path: &Path,
        events_path: &Path,
        start_path: &Path,
        start_line: u32,
    ) -> PyResult<()> {
        TraceWriter::begin_writing_trace_metadata(&mut self.writer, meta_path)
            .map_err(to_py_err)?;
        TraceWriter::begin_writing_trace_paths(&mut self.writer, paths_path).map_err(to_py_err)?;
        TraceWriter::begin_writing_trace_events(&mut self.writer, events_path)
            .map_err(to_py_err)?;
        TraceWriter::start(&mut self.writer, start_path, Line(start_line as i64));
        Ok(())
    }

    /// Return true when tracing is active; may become true on first event
    /// from the activation file if configured.
    fn ensure_started<'py>(&mut self, py: Python<'py>, code: &CodeObjectWrapper) {
        if self.started || self.activation_done {
            return;
        }
        if let Some(activation) = &self.activation_path {
            if let Ok(filename) = code.filename(py) {
                let f = Path::new(filename);
                //NOTE(Tzanko): We expect that code.filename contains an absolute path. If it turns out that this is sometimes not the case
                //we will investigate. For we won't do additional conversions here.
                // If there are issues the fool-proof solution is to use fs::canonicalize which needs to do syscalls
                if f == activation {
                    self.started = true;
                    self.activation_code_id = Some(code.id());
                    log::debug!(
                        "[RuntimeTracer] activated on enter: {}",
                        activation.display()
                    );
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
            return ValueRecord::String {
                text: s,
                type_id: ty,
            };
        }

        // Fallback to Raw string representation
        let ty = TraceWriter::ensure_type_id(&mut self.writer, TypeKind::Raw, "Object");
        match v.str() {
            Ok(s) => ValueRecord::Raw {
                r: s.to_string_lossy().into_owned(),
                type_id: ty,
            },
            Err(_) => ValueRecord::Error {
                msg: "<unrepr>".to_string(),
                type_id: ty,
            },
        }
    }

    fn ensure_function_id(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> PyResult<runtime_tracing::FunctionId> {
        //TODO AI! current runtime_tracer logic expects that `name` is unique and is used as a key for the function.
        //This is wrong. We need to write a test that exposes this issue
        let name = code.qualname(py)?;
        let filename = code.filename(py)?;
        let first_line = code.first_line(py)?;
        Ok(TraceWriter::ensure_function_id(
            &mut self.writer,
            name,
            Path::new(filename),
            Line(first_line as i64),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracer::{install_tracer, uninstall_tracer, EventSet, MonitoringEvents, Tracer};
    use pyo3::types::PyModule;
    use runtime_tracing::{TraceEventsFileFormat, TraceLowLevelEvent};
    use std::collections::{HashMap, HashSet};
    use std::ffi::CString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    struct SharedTracer {
        inner: Arc<Mutex<RuntimeTracer>>,
    }

    impl Tracer for SharedTracer {
        fn interest(&self, events: &MonitoringEvents) -> EventSet {
            let guard = self.inner.lock().unwrap();
            guard.interest(events)
        }

        fn on_py_start(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32) {
            let mut guard = self.inner.lock().unwrap();
            guard.on_py_start(py, code, offset);
        }

        fn on_line(&mut self, py: Python<'_>, code: &CodeObjectWrapper, lineno: u32) {
            let mut guard = self.inner.lock().unwrap();
            guard.on_line(py, code, lineno);
        }

        fn on_py_return(
            &mut self,
            py: Python<'_>,
            code: &CodeObjectWrapper,
            offset: i32,
            retval: &Bound<'_, PyAny>,
        ) {
            let mut guard = self.inner.lock().unwrap();
            guard.on_py_return(py, code, offset, retval);
        }

        fn flush(&mut self, py: Python<'_>) -> PyResult<()> {
            let mut guard = self.inner.lock().unwrap();
            guard.flush(py)
        }

        fn finish(&mut self, py: Python<'_>) -> PyResult<()> {
            let mut guard = self.inner.lock().unwrap();
            guard.finish(py)
        }
    }

    fn run_script(
        py: Python<'_>,
        code: &str,
        file_stem: &str,
    ) -> PyResult<(Vec<TraceLowLevelEvent>, PathBuf)> {
        let tmp_dir = tempdir().unwrap();
        let file_path = tmp_dir.path().join(format!("{file_stem}.py"));
        fs::write(&file_path, code).unwrap();

        let mut tracer = RuntimeTracer::new("test_program", &[], TraceEventsFileFormat::Json, None);

        let meta_path = tmp_dir.path().join("trace_metadata.json");
        let paths_path = tmp_dir.path().join("trace_paths.json");
        let events_path = tmp_dir.path().join("trace.json");
        tracer.begin(&meta_path, &paths_path, &events_path, &file_path, 1)?;

        let shared = Arc::new(Mutex::new(tracer));
        install_tracer(
            py,
            Box::new(SharedTracer {
                inner: shared.clone(),
            }),
        )?;

        let code_cstr = CString::new(code).unwrap();
        let filename_cstr = CString::new(file_path.to_str().unwrap()).unwrap();
        let module_cstr = CString::new("scenario").unwrap();
        PyModule::from_code(py, &code_cstr, &filename_cstr, &module_cstr)?;

        uninstall_tracer(py)?;

        let events = shared.lock().unwrap().writer.events.clone();
        Ok((events, file_path))
    }

    fn collect_line_names(
        events: &[TraceLowLevelEvent],
        target_path: &Path,
    ) -> HashMap<i64, Vec<HashSet<String>>> {
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut variable_names: Vec<String> = Vec::new();
        let mut current: Option<(PathBuf, i64, HashSet<String>)> = None;
        let mut result: HashMap<i64, Vec<HashSet<String>>> = HashMap::new();

        for event in events {
            match event {
                TraceLowLevelEvent::Path(p) => paths.push(p.clone()),
                TraceLowLevelEvent::VariableName(name) => variable_names.push(name.clone()),
                TraceLowLevelEvent::Step(step) => {
                    if let Some((path, line, names)) = current.take() {
                        if path == target_path {
                            result.entry(line).or_default().push(names);
                        }
                    }
                    let path = paths
                        .get(step.path_id.0)
                        .cloned()
                        .unwrap_or_else(|| PathBuf::from("<unknown>"));
                    current = Some((path, step.line.0, HashSet::new()));
                }
                TraceLowLevelEvent::Value(full) => {
                    if let Some((_, _, names)) = current.as_mut() {
                        if let Some(name) = variable_names.get(full.variable_id.0) {
                            names.insert(name.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        if let Some((path, line, names)) = current {
            if path == target_path {
                result.entry(line).or_default().push(names);
            }
        }

        result
    }

    fn assert_line_contains(
        map: &HashMap<i64, Vec<HashSet<String>>>,
        line: i64,
        expected: &[&str],
    ) {
        let entries = map
            .get(&line)
            .unwrap_or_else(|| panic!("no snapshots captured for line {line}"));
        let mut satisfied = false;
        for vars in entries {
            if expected.iter().all(|name| vars.contains(*name)) {
                satisfied = true;
                break;
            }
        }
        assert!(
            satisfied,
            "line {line} did not contain all expected names {:?}; captured {:?}",
            expected, entries
        );
    }

    fn assert_line_missing(map: &HashMap<i64, Vec<HashSet<String>>>, line: i64, name: &str) {
        if let Some(entries) = map.get(&line) {
            for vars in entries {
                assert!(
                    !vars.contains(name),
                    "expected name '{name}' to be absent on line {line}, but captured {:?}",
                    vars
                );
            }
        }
    }

    #[test]
    fn captures_simple_function_locals() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "def simple_function(x):\n    a = 1\n    b = a + x\n    return a, b\n\nresult = simple_function(5)\n";
            let (events, path) = run_script(py, code, "simple_function")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 2, &["x", "a"]);
            assert_line_contains(&line_map, 3, &["x", "a", "b"]);
            assert_line_contains(&line_map, 4, &["x", "a", "b"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_closure_and_nonlocals() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "def outer_func(x):\n    y = 1\n    def inner_func(z):\n        nonlocal y\n        w = x + y + z\n        y = w\n        return w\n    total = inner_func(5)\n    return y, total\nresult = outer_func(2)\n";
            let (events, path) = run_script(py, code, "nested_functions")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 5, &["x", "y", "z", "w"]);
            assert_line_contains(&line_map, 6, &["x", "y", "z", "w"]);
            assert_line_contains(&line_map, 8, &["x", "y", "total"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_globals_in_function_and_module_scope() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "GLOBAL_VAL = 10\ncounter = 0\n\ndef global_test():\n    local_copy = GLOBAL_VAL\n    global counter\n    counter += 1\n    return local_copy, counter\n\nbefore = counter\nresult = global_test()\nafter = counter\n";
            let (events, path) = run_script(py, code, "globals")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 5, &["local_copy", "GLOBAL_VAL"]);
            assert_line_contains(&line_map, 7, &["local_copy", "counter", "GLOBAL_VAL"]);
            assert_line_contains(&line_map, 10, &["before", "counter"]);
            assert_line_contains(&line_map, 12, &["after", "counter", "result"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_class_body_and_metaclass_variables() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "CONSTANT = 42\n\nclass MetaCounter(type):\n    count = 0\n    def __init__(cls, name, bases, attrs):\n        MetaCounter.count += 1\n        super().__init__(name, bases, attrs)\n\nclass Sample(metaclass=MetaCounter):\n    a = 10\n    b = a + 5\n    print(a, b, CONSTANT)\n    def method(self):\n        return self.a + self.b\n\ninstances = MetaCounter.count\n";
            let (events, path) = run_script(py, code, "class_scope")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 6, &["MetaCounter", "count", "cls", "name"]);
            assert_line_contains(&line_map, 11, &["a", "b"]);
            assert_line_contains(&line_map, 12, &["a", "b", "CONSTANT"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_comprehension_and_lambda_scopes() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "factor = 2\ndouble = lambda y: y * factor\nlambda_result = double(3)\nsquares = [n**2 for n in range(3)]\nscaled_set = {n * factor for n in range(3)}\nmapping = {n: n*factor for n in range(3)}\ngen_exp = (n * factor for n in range(3))\nresult_list = list(gen_exp)\n";
            let (events, path) = run_script(py, code, "comprehensions")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 2, &["y", "factor"]);
            assert_line_contains(&line_map, 4, &["n", "factor"]);
            assert_line_contains(&line_map, 5, &["n", "factor"]);
            assert_line_contains(&line_map, 6, &["n", "factor"]);
            assert_line_contains(&line_map, 7, &["n", "factor"]);
            assert_line_missing(&line_map, 8, "n");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_generators_and_coroutines() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "def counter_gen(n):\n    total = 0\n    for i in range(n):\n        total += i\n        yield total\n    return total\n\nimport asyncio\nasync def async_sum(data):\n    total = 0\n    for x in data:\n        total += x\n        await asyncio.sleep(0)\n    return total\n\ngen = counter_gen(3)\ngen_results = list(gen)\ncoroutine_result = asyncio.run(async_sum([1, 2, 3]))\n";
            let (events, path) = run_script(py, code, "generators")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 4, &["total", "i"]);
            assert_line_contains(&line_map, 5, &["total", "i"]);
            assert_line_contains(&line_map, 13, &["total", "x"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_exception_and_with_scopes() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "def exception_and_with_demo(x):\n    try:\n        inv = 10 / x\n    except ZeroDivisionError as e:\n        error_msg = f\"Error: {e}\"\n    else:\n        inv += 1\n    finally:\n        final_flag = True\n\n    with open(__file__, 'r') as f:\n        first_line = f.readline()\n    return locals()\n\nresult1 = exception_and_with_demo(0)\nresult2 = exception_and_with_demo(5)\n";
            let (events, path) = run_script(py, code, "exceptions")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 5, &["e", "error_msg"]);
            assert_line_contains(&line_map, 7, &["inv"]);
            if let Some(entries) = line_map.get(&9) {
                for vars in entries {
                    assert!(!vars.contains("e"));
                }
            }
            assert_line_contains(&line_map, 11, &["f", "first_line"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_decorator_scopes() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "setting = \"Hello\"\n\ndef my_decorator(func):\n    def wrapper(*args, **kwargs):\n        print(\"Decorator wrapping with setting:\", setting)\n        return func(*args, **kwargs)\n    return wrapper\n\n@my_decorator\ndef greet(name):\n    message = f\"Hi, {name}\"\n    return message\n\noutput = greet(\"World\")\n";
            let (events, path) = run_script(py, code, "decorators")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 5, &["args", "kwargs", "setting"]);
            assert_line_contains(&line_map, 10, &["name", "message"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_dynamic_exec_and_eval() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "expr_code = \"dynamic_var = 99\"\nexec(expr_code)\ncheck = dynamic_var + 1\n\ndef eval_test():\n    value = 10\n    formula = \"value * 2\"\n    result = eval(formula)\n    return result\n\nout = eval_test()\n";
            let (events, path) = run_script(py, code, "dynamic_exec")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 3, &["dynamic_var", "check"]);
            assert_line_contains(&line_map, 7, &["value", "formula", "result"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_import_visibility() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "import math\n\ndef import_test():\n    import os\n    constant = math.pi\n    cwd = os.getcwd()\n    return constant, cwd\n\nval, path = import_test()\n";
            let (events, path) = run_script(py, code, "imports")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 5, &["constant", "math", "os"]);
            assert_line_contains(&line_map, 6, &["cwd", "os"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn captures_builtin_usage_without_listing_builtins() {
        Python::with_gil(|py| -> PyResult<()> {
            let code = "def builtins_test(seq):\n    n = len(seq)\n    m = max(seq)\n    return n, m\n\nresult = builtins_test([5, 3, 7])\n";
            let (events, path) = run_script(py, code, "builtins")?;
            let line_map = collect_line_names(&events, &path);
            assert_line_contains(&line_map, 2, &["seq", "n"]);
            assert_line_contains(&line_map, 3, &["seq", "m", "n"]);
            assert_line_missing(&line_map, 2, "len");
            assert_line_missing(&line_map, 3, "max");
            Ok(())
        })
        .unwrap();
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
        if !self.started {
            return;
        }
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
        if !self.started {
            return;
        }
        // Trace event entry
        if let Ok(fname) = code.filename(py) {
            log::debug!("[RuntimeTracer] on_line: {}:{}", fname, lineno);
        } else {
            log::debug!("[RuntimeTracer] on_line: <unknown>:{}", lineno);
        }
        if let Ok(filename) = code.filename(py) {
            TraceWriter::register_step(&mut self.writer, Path::new(filename), Line(lineno as i64));
        }
        if let Err(err) = self.capture_scope_variables(py, code) {
            log::warn!("[RuntimeTracer] failed to capture variables: {}", err);
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
        if !self.started {
            return;
        }
        // Trace event entry
        match (code.filename(py), code.qualname(py)) {
            (Ok(fname), Ok(qname)) => {
                log::debug!("[RuntimeTracer] on_py_return: {} ({})", qname, fname)
            }
            _ => log::debug!("[RuntimeTracer] on_py_return"),
        }
        // Determine whether this is the activation owner's return
        let is_activation_return = self
            .activation_code_id
            .map(|id| id == code.id())
            .unwrap_or(false);

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

impl RuntimeTracer {
    fn capture_scope_variables(
        &mut self,
        py: Python<'_>,
        code: &CodeObjectWrapper,
    ) -> PyResult<()> {
        unsafe {
            let tstate = pyo3::ffi::PyThreadState_Get();
            if tstate.is_null() {
                return Ok(());
            }

            let mut frame_ptr = pyo3::ffi::PyThreadState_GetFrame(tstate);
            if frame_ptr.is_null() {
                return Ok(());
            }

            let target_code = code.as_bound(py).as_ptr() as *mut pyo3::ffi::PyCodeObject;

            while !frame_ptr.is_null() {
                let frame_code = pyo3::ffi::PyFrame_GetCode(frame_ptr);
                if frame_code == target_code {
                    let frame_obj =
                        Py::from_borrowed_ptr(py, frame_ptr.cast::<pyo3::ffi::PyObject>());
                    let frame_bound = frame_obj.bind(py);
                    let frame = frame_bound.downcast::<PyFrame>()?;
                    self.record_frame_variables(py, frame)?;
                    return Ok(());
                }
                frame_ptr = pyo3::ffi::PyFrame_GetBack(frame_ptr);
            }
        }
        Ok(())
    }

    fn record_frame_variables(
        &mut self,
        py: Python<'_>,
        frame: &Bound<'_, PyFrame>,
    ) -> PyResult<()> {
        let locals_any = frame.getattr("f_locals")?;
        let locals_mapping = locals_any.downcast::<PyMapping>()?;
        let locals_snapshot = PyDict::new(py);
        locals_snapshot.update(&locals_mapping)?;

        let globals_any = frame.getattr("f_globals")?;
        let globals_dict = globals_any.downcast::<PyDict>()?;

        let mut seen = HashSet::new();
        self.record_dict(py, &locals_snapshot, &mut seen);
        self.record_dict(py, globals_dict, &mut seen);
        Ok(())
    }

    fn record_dict(
        &mut self,
        py: Python<'_>,
        dictionary: &Bound<'_, PyDict>,
        seen: &mut HashSet<String>,
    ) {
        for (key, value) in dictionary.iter() {
            let Ok(name_obj) = key.str() else { continue };
            let Ok(name) = name_obj.to_str() else {
                continue;
            };
            if name == "__builtins__" {
                continue;
            }
            let owned_name = name.to_string();
            if !seen.insert(owned_name.clone()) {
                continue;
            }
            let value_record = self.encode_value(py, &value);
            TraceWriter::register_variable_with_full_value(
                &mut self.writer,
                &owned_name,
                value_record,
            );
        }
    }
}
