use codetracer_python_recorder::{CodeObjectWrapper, CODE_REGISTRY};
use pyo3::prelude::*;
use pyo3::types::{PyCode, PyModule};
use std::ffi::CString;

#[test]
fn wrapper_basic_attributes() {
    Python::with_gil(|py| {
        let src = CString::new("def f(x):\n    return x + 1\n").unwrap();
        let filename = CString::new("<string>").unwrap();
        let module = CString::new("m").unwrap();
        let m = PyModule::from_code(py, src.as_c_str(), filename.as_c_str(), module.as_c_str()).unwrap();
        let func = m.getattr("f").unwrap();
        let code: Bound<'_, PyCode> = func
            .getattr("__code__")
            .unwrap()
            .clone()
            .downcast_into()
            .unwrap();
        let wrapper = CodeObjectWrapper::new(py, &code);
        assert_eq!(wrapper.arg_count(py).unwrap(), 1);
        assert_eq!(wrapper.filename(py).unwrap(), "<string>");
        assert_eq!(wrapper.qualname(py).unwrap(), "f");
        assert!(wrapper.flags(py).unwrap() > 0);
    });
}

#[test]
fn wrapper_line_for_offset() {
    Python::with_gil(|py| {
        let src = CString::new("def g():\n    a = 1\n    b = 2\n    return a + b\n").unwrap();
        let filename = CString::new("<string>").unwrap();
        let module = CString::new("m2").unwrap();
        let m = PyModule::from_code(py, src.as_c_str(), filename.as_c_str(), module.as_c_str()).unwrap();
        let func = m.getattr("g").unwrap();
        let code: Bound<'_, PyCode> = func
            .getattr("__code__")
            .unwrap()
            .clone()
            .downcast_into()
            .unwrap();
        let wrapper = CodeObjectWrapper::new(py, &code);
        let lines = code.call_method0("co_lines").unwrap();
        let iter = lines.try_iter().unwrap();
        let mut last_line = None;
        for item in iter {
            let (start, _end, line): (u32, u32, Option<u32>) = item.unwrap().extract().unwrap();
            if let Some(l) = line {
                assert_eq!(wrapper.line_for_offset(py, start).unwrap(), Some(l));
                last_line = Some(l);
            }
        }
        assert_eq!(wrapper.line_for_offset(py, 10_000).unwrap(), last_line);
    });
}

#[test]
fn registry_reuses_wrappers() {
    Python::with_gil(|py| {
        let src = CString::new("def h():\n    return 0\n").unwrap();
        let filename = CString::new("<string>").unwrap();
        let module = CString::new("m3").unwrap();
        let m =
            PyModule::from_code(py, src.as_c_str(), filename.as_c_str(), module.as_c_str())
                .unwrap();
        let func = m.getattr("h").unwrap();
        let code: Bound<'_, PyCode> = func
            .getattr("__code__")
            .unwrap()
            .clone()
            .downcast_into()
            .unwrap();
        let w1 = CODE_REGISTRY.get_or_insert(py, &code);
        let w2 = CODE_REGISTRY.get_or_insert(py, &code);
        assert!(std::sync::Arc::ptr_eq(&w1, &w2));
        CODE_REGISTRY.remove(w1.id());
    });
}
