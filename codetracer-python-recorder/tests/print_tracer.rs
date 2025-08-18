use codetracer_python_recorder::{install_tracer, uninstall_tracer, EventMask, Tracer};
use pyo3::prelude::*;
use pyo3::ffi::PyFrameObject;
use std::ffi::CString;

struct PrintTracer;

impl Tracer for PrintTracer {
    fn interest(&self) -> EventMask {
        EventMask::CALL
    }

    fn on_call(&mut self, _py: Python<'_>, _frame: *mut PyFrameObject) {
        println!("call event");
    }
}

#[test]
fn tracer_prints_on_call() {
    Python::with_gil(|py| {
        install_tracer(py, Box::new(PrintTracer)).unwrap();
        let code = CString::new("def foo():\n    return 1\nfoo()").unwrap();
        py.run(&code, None, None).unwrap();
        uninstall_tracer(py).unwrap();
    });
}

