use super::common::OutputProxy;
use crate::runtime::io_capture::events::IoStream;
use crate::runtime::io_capture::fd_mirror::MirrorLedgers;
use pyo3::prelude::*;
use pyo3::types::PyAny;
use std::sync::Arc;

macro_rules! define_output_proxy {
    ($name:ident, $stream:expr) => {
        #[pyclass(module = "codetracer_python_recorder.runtime")]
        pub struct $name {
            inner: OutputProxy,
        }

        impl $name {
            pub fn new(
                original: PyObject,
                sink: Arc<dyn crate::runtime::io_capture::events::ProxySink>,
                ledgers: Option<MirrorLedgers>,
            ) -> Self {
                Self {
                    inner: OutputProxy::new(original, sink, $stream, ledgers),
                }
            }
        }

        #[pymethods]
        impl $name {
            fn write(&self, py: Python<'_>, text: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
                self.inner.write(py, text)
            }

            fn writelines(&self, py: Python<'_>, lines: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
                self.inner.writelines(py, lines)
            }

            fn flush(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
                self.inner.flush(py)
            }

            fn fileno(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
                self.inner.fileno(py)
            }

            fn isatty(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
                self.inner.isatty(py)
            }

            fn close(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
                self.inner.close(py)
            }

            #[getter]
            fn encoding(&self, py: Python<'_>) -> PyResult<PyObject> {
                self.inner.getattr(py, "encoding")
            }

            #[getter]
            fn errors(&self, py: Python<'_>) -> PyResult<PyObject> {
                self.inner.getattr(py, "errors")
            }

            #[getter]
            fn buffer(&self, py: Python<'_>) -> PyResult<PyObject> {
                self.inner.getattr(py, "buffer")
            }

            fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
                self.inner.getattr(py, name)
            }
        }
    };
}

define_output_proxy!(LineAwareStdout, IoStream::Stdout);
define_output_proxy!(LineAwareStderr, IoStream::Stderr);
