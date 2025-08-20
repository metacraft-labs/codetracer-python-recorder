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

