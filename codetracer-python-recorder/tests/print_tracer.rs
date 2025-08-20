use codetracer_python_recorder::{install_tracer, uninstall_tracer, EventSet, Tracer};
use codetracer_python_recorder::tracer::{MonitoringEvents, events_union};
use pyo3::prelude::*;
use std::ffi::CString;
use std::sync::atomic::{AtomicUsize, Ordering};

static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

struct PrintTracer;

impl Tracer for PrintTracer {
    fn interest(&self, events:&MonitoringEvents) -> EventSet {
	events_union(&[events.CALL])
    }

    fn on_call(
        &mut self,
        _py: Python<'_>,
        _code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        _offset: i32,
        _callable: &pyo3::Bound<'_, pyo3::types::PyAny>,
        _arg0: Option<&pyo3::Bound<'_, pyo3::types::PyAny>>,
    ) {
        CALL_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn tracer_prints_on_call() {
    Python::with_gil(|py| {
        CALL_COUNT.store(0, Ordering::SeqCst);
        if let Err(e) = install_tracer(py, Box::new(PrintTracer)) {
            e.print(py);
            panic!("Install Tracer failed");
        }
        let code = CString::new("def foo():\n    return 1\nfoo()").expect("CString::new failed");
        if let Err(e) = py.run(code.as_c_str(), None, None) {
            e.print(py);
            uninstall_tracer(py).ok();
            panic!("Python raised an exception");
        }
        uninstall_tracer(py).unwrap();
        let count = CALL_COUNT.load(Ordering::SeqCst);
        assert!(count >= 1, "expected at least one CALL event, got {}", count);
    });
}

static LINE_COUNT: AtomicUsize = AtomicUsize::new(0);
static INSTRUCTION_COUNT: AtomicUsize = AtomicUsize::new(0);
static JUMP_COUNT: AtomicUsize = AtomicUsize::new(0);
static BRANCH_COUNT: AtomicUsize = AtomicUsize::new(0);
static PY_START_COUNT: AtomicUsize = AtomicUsize::new(0);
static PY_RESUME_COUNT: AtomicUsize = AtomicUsize::new(0);
static PY_RETURN_COUNT: AtomicUsize = AtomicUsize::new(0);
static PY_YIELD_COUNT: AtomicUsize = AtomicUsize::new(0);
static PY_THROW_COUNT: AtomicUsize = AtomicUsize::new(0);
static PY_UNWIND_COUNT: AtomicUsize = AtomicUsize::new(0);
static RAISE_COUNT: AtomicUsize = AtomicUsize::new(0);
static RERAISE_COUNT: AtomicUsize = AtomicUsize::new(0);
static EXCEPTION_HANDLED_COUNT: AtomicUsize = AtomicUsize::new(0);
static C_RETURN_COUNT: AtomicUsize = AtomicUsize::new(0);
static C_RAISE_COUNT: AtomicUsize = AtomicUsize::new(0);

struct CountingTracer;

fn offset_to_line(code: &pyo3::Bound<'_, pyo3::types::PyAny>, offset: i32) -> Option<i32> {
    if offset < 0 {
        return None;
    }
    let lines_iter = code.call_method0("co_lines").ok()?;
    let iter = lines_iter.try_iter().ok()?;
    for line_info in iter {
        let line_info = line_info.ok()?;
        let (start, end, line): (i32, i32, i32) = line_info.extract().ok()?;
        if offset >= start && offset < end {
            return Some(line);
        }
    }
    None
}

impl Tracer for CountingTracer {
    fn interest(&self, events: &MonitoringEvents) -> EventSet {
        events_union(&[
            events.CALL,
            events.LINE,
            events.INSTRUCTION,
            events.JUMP,
            events.BRANCH,
            events.PY_START,
            events.PY_RESUME,
            events.PY_RETURN,
            events.PY_YIELD,
            events.PY_THROW,
            events.PY_UNWIND,
            events.RAISE,
            events.RERAISE,
            events.EXCEPTION_HANDLED,
            events.C_RETURN,
            events.C_RAISE,
        ])
    }

    fn on_line(
        &mut self,
        _py: Python<'_>,
        _code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        lineno: u32,
    ) {
        LINE_COUNT.fetch_add(1, Ordering::SeqCst);
        println!("LINE at {}", lineno);
    }

    fn on_instruction(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
    ) {
        INSTRUCTION_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("INSTRUCTION at {}", line);
        }
    }

    fn on_jump(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _destination_offset: i32,
    ) {
        JUMP_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("JUMP at {}", line);
        }
    }

    fn on_branch(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _destination_offset: i32,
    ) {
        BRANCH_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("BRANCH at {}", line);
        }
    }

    fn on_py_start(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
    ) {
        PY_START_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("PY_START at {}", line);
        }
    }

    fn on_py_resume(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
    ) {
        PY_RESUME_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("PY_RESUME at {}", line);
        }
    }

    fn on_py_return(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _retval: &pyo3::Bound<'_, pyo3::types::PyAny>,
    ) {
        PY_RETURN_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("PY_RETURN at {}", line);
        }
    }

    fn on_py_yield(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _retval: &pyo3::Bound<'_, pyo3::types::PyAny>,
    ) {
        PY_YIELD_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("PY_YIELD at {}", line);
        }
    }

    fn on_py_throw(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _exception: &pyo3::Bound<'_, pyo3::types::PyAny>,
    ) {
        PY_THROW_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("PY_THROW at {}", line);
        }
    }

    fn on_py_unwind(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _exception: &pyo3::Bound<'_, pyo3::types::PyAny>,
    ) {
        PY_UNWIND_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("PY_UNWIND at {}", line);
        }
    }

    fn on_raise(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _exception: &pyo3::Bound<'_, pyo3::types::PyAny>,
    ) {
        RAISE_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("RAISE at {}", line);
        }
    }

    fn on_reraise(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _exception: &pyo3::Bound<'_, pyo3::types::PyAny>,
    ) {
        RERAISE_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("RERAISE at {}", line);
        }
    }

    fn on_exception_handled(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _exception: &pyo3::Bound<'_, pyo3::types::PyAny>,
    ) {
        EXCEPTION_HANDLED_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("EXCEPTION_HANDLED at {}", line);
        }
    }

    fn on_c_return(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _callable: &pyo3::Bound<'_, pyo3::types::PyAny>,
        _arg0: Option<&pyo3::Bound<'_, pyo3::types::PyAny>>,
    ) {
        C_RETURN_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("C_RETURN at {}", line);
        }
    }

    fn on_c_raise(
        &mut self,
        _py: Python<'_>,
        code: &pyo3::Bound<'_, pyo3::types::PyAny>,
        offset: i32,
        _callable: &pyo3::Bound<'_, pyo3::types::PyAny>,
        _arg0: Option<&pyo3::Bound<'_, pyo3::types::PyAny>>,
    ) {
        C_RAISE_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Some(line) = offset_to_line(code, offset) {
            println!("C_RAISE at {}", line);
        }
    }
}

#[test]
fn tracer_handles_all_events() {
    Python::with_gil(|py| {
        LINE_COUNT.store(0, Ordering::SeqCst);
        INSTRUCTION_COUNT.store(0, Ordering::SeqCst);
        JUMP_COUNT.store(0, Ordering::SeqCst);
        BRANCH_COUNT.store(0, Ordering::SeqCst);
        PY_START_COUNT.store(0, Ordering::SeqCst);
        PY_RESUME_COUNT.store(0, Ordering::SeqCst);
        PY_RETURN_COUNT.store(0, Ordering::SeqCst);
        PY_YIELD_COUNT.store(0, Ordering::SeqCst);
        PY_THROW_COUNT.store(0, Ordering::SeqCst);
        PY_UNWIND_COUNT.store(0, Ordering::SeqCst);
        RAISE_COUNT.store(0, Ordering::SeqCst);
        RERAISE_COUNT.store(0, Ordering::SeqCst);
        EXCEPTION_HANDLED_COUNT.store(0, Ordering::SeqCst);
        C_RETURN_COUNT.store(0, Ordering::SeqCst);
        C_RAISE_COUNT.store(0, Ordering::SeqCst);
        if let Err(e) = install_tracer(py, Box::new(CountingTracer)) {
            e.print(py);
            panic!("Install Tracer failed");
        }
        let code = CString::new(r#"
def test_all():
    x = 0
    if x == 0:
        x += 1
    for i in range(1):
        x += i
    def foo():
        return 1
    foo()
    try:
        raise ValueError("err")
    except ValueError:
        pass
    def gen():
        try:
            yield 1
            yield 2
        except ValueError:
            pass
    g = gen()
    next(g)
    next(g)
    try:
        g.throw(ValueError())
    except StopIteration:
        pass
    for _ in []:
        pass
    len("abc")
    try:
        int("a")
    except ValueError:
        pass
    def f_unwind():
        raise KeyError
    try:
        f_unwind()
    except KeyError:
        pass
    try:
        try:
            raise OSError()
        except OSError:
            raise
    except OSError:
        pass
test_all()
"#).expect("CString::new failed");
        if let Err(e) = py.run(code.as_c_str(), None, None) {
            e.print(py);
            uninstall_tracer(py).ok();
            panic!("Python raised an exception");
        }
        uninstall_tracer(py).unwrap();
        assert!(LINE_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one LINE event, got {}", LINE_COUNT.load(Ordering::SeqCst));
        assert!(INSTRUCTION_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one INSTRUCTION event, got {}", INSTRUCTION_COUNT.load(Ordering::SeqCst));
        assert!(JUMP_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one JUMP event, got {}", JUMP_COUNT.load(Ordering::SeqCst));
        assert!(BRANCH_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one BRANCH event, got {}", BRANCH_COUNT.load(Ordering::SeqCst));
        assert!(PY_START_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one PY_START event, got {}", PY_START_COUNT.load(Ordering::SeqCst));
        assert!(PY_RESUME_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one PY_RESUME event, got {}", PY_RESUME_COUNT.load(Ordering::SeqCst));
        assert!(PY_RETURN_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one PY_RETURN event, got {}", PY_RETURN_COUNT.load(Ordering::SeqCst));
        assert!(PY_YIELD_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one PY_YIELD event, got {}", PY_YIELD_COUNT.load(Ordering::SeqCst));
        assert!(PY_THROW_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one PY_THROW event, got {}", PY_THROW_COUNT.load(Ordering::SeqCst));
        assert!(PY_UNWIND_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one PY_UNWIND event, got {}", PY_UNWIND_COUNT.load(Ordering::SeqCst));
        assert!(RAISE_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one RAISE event, got {}", RAISE_COUNT.load(Ordering::SeqCst));
        assert!(RERAISE_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one RERAISE event, got {}", RERAISE_COUNT.load(Ordering::SeqCst));
        assert!(EXCEPTION_HANDLED_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one EXCEPTION_HANDLED event, got {}", EXCEPTION_HANDLED_COUNT.load(Ordering::SeqCst));
        assert!(C_RETURN_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one C_RETURN event, got {}", C_RETURN_COUNT.load(Ordering::SeqCst));
        assert!(C_RAISE_COUNT.load(Ordering::SeqCst) >= 1, "expected at least one C_RAISE event, got {}", C_RAISE_COUNT.load(Ordering::SeqCst));
    });
}

