use codetracer_python_recorder::{install_tracer, uninstall_tracer, EventSet, Tracer, CodeObjectWrapper};
use codetracer_python_recorder::tracer::{MonitoringEvents, events_union};
use pyo3::prelude::*;
use std::ffi::CString;
use std::sync::atomic::{AtomicUsize, Ordering};

static CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

struct PrintTracer;

impl Tracer for PrintTracer {
    fn interest(&self, events: &MonitoringEvents) -> EventSet {
        events_union(&[events.CALL])
    }

    fn on_call(
        &mut self,
        _py: Python<'_>,
        _code: &CodeObjectWrapper,
        _offset: i32,
        _callable: &Bound<'_, PyAny>,
        _arg0: Option<&Bound<'_, PyAny>>,
    ) {
        CALL_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn tracer_prints_on_call() {
    Python::with_gil(|py| {
        CALL_COUNT.store(0, Ordering::SeqCst);
        uninstall_tracer(py).ok();
        install_tracer(py, Box::new(PrintTracer)).unwrap();
        let code = CString::new("def foo():\n    return 1\nfoo()").unwrap();
        py.run(code.as_c_str(), None, None).unwrap();
        uninstall_tracer(py).unwrap();
        assert!(CALL_COUNT.load(Ordering::SeqCst) >= 1);
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

    fn on_line(&mut self, _py: Python<'_>, _code: &CodeObjectWrapper, lineno: u32) {
        LINE_COUNT.fetch_add(1, Ordering::SeqCst);
        println!("LINE at {}", lineno);
    }

    fn on_instruction(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32) {
        INSTRUCTION_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("INSTRUCTION at {}", line);
        }
    }

    fn on_jump(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _dest: i32) {
        JUMP_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("JUMP at {}", line);
        }
    }

    fn on_branch(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _dest: i32) {
        BRANCH_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("BRANCH at {}", line);
        }
    }

    fn on_py_start(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32) {
        PY_START_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("PY_START at {}", line);
        }
    }

    fn on_py_resume(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32) {
        PY_RESUME_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("PY_RESUME at {}", line);
        }
    }

    fn on_py_return(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _retval: &Bound<'_, PyAny>) {
        PY_RETURN_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("PY_RETURN at {}", line);
        }
    }

    fn on_py_yield(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _retval: &Bound<'_, PyAny>) {
        PY_YIELD_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("PY_YIELD at {}", line);
        }
    }

    fn on_py_throw(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _exc: &Bound<'_, PyAny>) {
        PY_THROW_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("PY_THROW at {}", line);
        }
    }

    fn on_py_unwind(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _exc: &Bound<'_, PyAny>) {
        PY_UNWIND_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("PY_UNWIND at {}", line);
        }
    }

    fn on_raise(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _exc: &Bound<'_, PyAny>) {
        RAISE_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("RAISE at {}", line);
        }
    }

    fn on_reraise(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _exc: &Bound<'_, PyAny>) {
        RERAISE_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("RERAISE at {}", line);
        }
    }

    fn on_exception_handled(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _exc: &Bound<'_, PyAny>) {
        EXCEPTION_HANDLED_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("EXCEPTION_HANDLED at {}", line);
        }
    }

    fn on_c_return(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _call: &Bound<'_, PyAny>, _arg0: Option<&Bound<'_, PyAny>>) {
        C_RETURN_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
            println!("C_RETURN at {}", line);
        }
    }

    fn on_c_raise(&mut self, py: Python<'_>, code: &CodeObjectWrapper, offset: i32, _call: &Bound<'_, PyAny>, _arg0: Option<&Bound<'_, PyAny>>) {
        C_RAISE_COUNT.fetch_add(1, Ordering::SeqCst);
        if let Ok(Some(line)) = code.line_for_offset(py, offset as u32) {
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
        install_tracer(py, Box::new(CountingTracer)).unwrap();
        let code = CString::new(
"def test_all():\n    x = 0\n    if x == 0:\n        x += 1\n    for i in range(1):\n        x += i\n    def foo():\n        return 1\n    foo()\n    try:\n        raise ValueError('err')\n    except ValueError:\n        pass\n    def gen():\n        try:\n            yield 1\n            yield 2\n        except ValueError:\n            pass\n    g = gen()\n    next(g)\n    next(g)\n    try:\n        g.throw(ValueError())\n    except StopIteration:\n        pass\n    for _ in []:\n        pass\n    def gen2():\n        yield 1\n        return 2\n    for _ in gen2():\n        pass\n    len('abc')\n    try:\n        int('a')\n    except ValueError:\n        pass\n    def f_unwind():\n        raise KeyError\n    try:\n        f_unwind()\n    except KeyError:\n        pass\n    try:\n        try:\n            raise OSError()\n        except OSError:\n            raise\n    except OSError:\n        pass\n\ntest_all()\n").unwrap();
        py.run(code.as_c_str(), None, None).unwrap();
        uninstall_tracer(py).unwrap();
        assert!(LINE_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(INSTRUCTION_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(JUMP_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(BRANCH_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(PY_START_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(PY_RESUME_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(PY_RETURN_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(PY_YIELD_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(PY_THROW_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(PY_UNWIND_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(RAISE_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(RERAISE_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(EXCEPTION_HANDLED_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(C_RETURN_COUNT.load(Ordering::SeqCst) >= 1);
        assert!(C_RAISE_COUNT.load(Ordering::SeqCst) >= 1);
    });
}
