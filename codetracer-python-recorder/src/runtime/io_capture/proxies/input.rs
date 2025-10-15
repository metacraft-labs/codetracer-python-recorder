use super::common::{
    buffer_snapshot, current_thread_id, enter_reentrancy_guard, exit_reentrancy_guard, now,
};
use crate::runtime::io_capture::events::{IoOperation, IoStream, ProxyEvent, ProxySink};
use pyo3::exceptions::PyStopIteration;
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyAnyMethods};
use std::sync::Arc;

#[pyclass(module = "codetracer_python_recorder.runtime")]
pub struct LineAwareStdin {
    original: PyObject,
    sink: Arc<dyn ProxySink>,
}

impl LineAwareStdin {
    pub fn new(original: PyObject, sink: Arc<dyn ProxySink>) -> Self {
        Self { original, sink }
    }

    fn record(&self, py: Python<'_>, operation: IoOperation, payload: Vec<u8>) {
        let event = ProxyEvent {
            stream: IoStream::Stdin,
            operation,
            payload,
            thread_id: current_thread_id(),
            timestamp: now(),
            frame_id: None,
            path_id: None,
            line: None,
            path: None,
        };
        self.sink.record(py, event);
    }
}

#[pymethods]
impl LineAwareStdin {
    #[pyo3(signature = (size=None))]
    fn read(&self, py: Python<'_>, size: Option<isize>) -> PyResult<Py<PyAny>> {
        let entered = enter_reentrancy_guard();
        let result: PyResult<Py<PyAny>> = match size {
            Some(n) => self
                .original
                .call_method1(py, "read", (n,))
                .map(|value| value.into()),
            None => self
                .original
                .call_method1(py, "read", ())
                .map(|value| value.into()),
        };
        if entered {
            if let Ok(ref obj) = result {
                let bound = obj.bind(py);
                if let Ok(text) = bound.extract::<String>() {
                    if !text.is_empty() {
                        self.record(py, IoOperation::Read, text.into_bytes());
                    }
                }
            }
        }
        exit_reentrancy_guard(entered);
        result
    }

    #[pyo3(signature = (limit=None))]
    fn readline(&self, py: Python<'_>, limit: Option<isize>) -> PyResult<Py<PyAny>> {
        let entered = enter_reentrancy_guard();
        let result: PyResult<Py<PyAny>> = match limit {
            Some(n) => self
                .original
                .call_method1(py, "readline", (n,))
                .map(|value| value.into()),
            None => self
                .original
                .call_method1(py, "readline", ())
                .map(|value| value.into()),
        };
        if entered {
            if let Ok(ref obj) = result {
                let bound = obj.bind(py);
                if let Ok(text) = bound.extract::<String>() {
                    if !text.is_empty() {
                        self.record(py, IoOperation::ReadLine, text.into_bytes());
                    }
                }
            }
        }
        exit_reentrancy_guard(entered);
        result
    }

    fn readinto(&self, py: Python<'_>, buffer: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let entered = enter_reentrancy_guard();
        let args = (buffer.clone().unbind(),);
        let result: PyResult<Py<PyAny>> = self
            .original
            .call_method1(py, "readinto", args)
            .map(|value| value.into());
        if entered {
            if let Ok(ref obj) = result {
                if let Some(mut bytes) = buffer_snapshot(buffer) {
                    if let Ok(count) = obj.bind(py).extract::<usize>() {
                        let count = count.min(bytes.len());
                        if count > 0 {
                            bytes.truncate(count);
                            self.record(py, IoOperation::ReadInto, bytes);
                        }
                    }
                }
            }
        }
        exit_reentrancy_guard(entered);
        result
    }

    fn fileno(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.original
            .call_method1(py, "fileno", ())
            .map(|value| value.into())
    }

    fn isatty(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.original
            .call_method1(py, "isatty", ())
            .map(|value| value.into())
    }

    fn close(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.original
            .call_method1(py, "close", ())
            .map(|value| value.into())
    }

    fn __iter__(slf: PyRef<Self>) -> Py<LineAwareStdin> {
        slf.into()
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let line = self.readline(py, None)?;
        if line.bind(py).extract::<String>()?.is_empty() {
            Err(PyStopIteration::new_err(()))
        } else {
            Ok(Some(line))
        }
    }

    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        self.original.bind(py).getattr(name).map(|obj| obj.unbind())
    }
}
